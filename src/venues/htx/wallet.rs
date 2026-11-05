//! Файл: venues/htx/wallet.rs
//! Что делает: справочник монет и сетей HTX с кэшем в памяти не старше venue.networks_fresh_ms (решение 118): весь
//! список — GET /v2/reference/currencies (+ /v1/settings/common/chains для адреса контракта, если задано его поле;
//! ответы большие — разбор вне рабочих потоков, за `Arc` без копий, решение 131); одна монета — те же запросы с
//! ?currency= (решение 113); имя сети HTX (chain) по сети ядра — для вывода и адреса депозита; стоимость вывода.
//! Кто вызывает: mod.rs (функции контракта), transfers.rs, deposit_addr.rs.
//! От чего зависит: rest.rs, networks_parse.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [htx] — сеть BSC ядра (baseChainProtocol по живому ответу; по умолчанию нет),
//! venue.contract_field [htx] — поле адреса контракта в Get Chains Information (ac или ca — по живому ответу; не задано —
//! адресов нет, монеты HTX не сопоставляются), venue.deposit_min_absent_ok, venue.networks_fresh_ms (необязательная).
//!
//! Официально: APIv2 Currency & Chains GET /v2/reference/currencies?currency= и Get Chains Information
//! GET /v1/settings/common/chains?currency= (https://huobiapi.github.io/docs/spot/v1/en/, раздел Reference Data) —
//! поля в шапке networks_parse.rs. Без currency — все монеты.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Method;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::{Ctx, currency};
use super::errors::Op;
use super::networks_parse::{Contracts, Parsed, contracts, parse};
use super::rest::{self, Params, Req};

/// Путь справочника монет и сетей.
const CURRENCIES: &str = "/v2/reference/currencies";
/// Путь справочника сетей (адрес контракта).
const CHAINS: &str = "/v1/settings/common/chains";

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    /// Последнее записанное в журнал — пишем только при изменении.
    told: Arc<RwLock<String>>,
}

/// Все монеты HTX и их сети. Ошибки: настройки, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let fresh = ctx
        .int("venue.networks_fresh_ms")
        .ok()
        .map(Duration::from_millis);
    if let Some(f) = fresh {
        let c = nets.cache.read().unwrap_or_else(|p| p.into_inner());
        if let Some((_, l)) = c.as_ref().filter(|(at, _)| at.elapsed() < f) {
            return Ok(Arc::unwrap_or_clone(l.clone()));
        }
    }
    let p = load(ctx, vec![]).await?;
    note(ctx, Some(nets), &p).await;
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(Arc::unwrap_or_clone(list))
}

/// Сети одной монеты — запросами с ?currency= (решение 113). Ошибки: как coin_networks; монеты нет — пусто.
pub async fn coin_networks_of(ctx: &Ctx, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(one(ctx, ticker).await?.list)
}

/// chain HTX монеты по сети ядра (свежий справочник одной монеты). Ошибка NotConfigured — такой сети у монеты нет.
pub async fn chain_of(ctx: &Ctx, ticker: &str, network: &str) -> GatewayResult<String> {
    let p = one(ctx, ticker).await?;
    p.chains
        .into_iter()
        .find(|(t, n, _)| t == ticker && n == network)
        .map(|(_, _, c)| c)
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!("У {ticker} нет сети {network} в справочнике HTX"),
            ))
        })
}

/// Справочник одной монеты (обе части). Ошибки: как load.
async fn one(ctx: &Ctx, ticker: &str) -> GatewayResult<Parsed> {
    let p = load(ctx, vec![("currency", currency(ticker))]).await?;
    note(ctx, None, &p).await;
    Ok(p)
}

/// Что делает: запросы справочника (монеты + сети с адресом контракта, если задано поле) и разбор вне рабочих потоков.
/// Принимает: параметры (пусто — все монеты). Ошибки: настройки, связь, формат.
async fn load(ctx: &Ctx, q: Params) -> GatewayResult<Parsed> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let field = ctx.text("venue.contract_field").ok();
    let data = heavy(ctx, CURRENCIES, q.clone()).await?;
    let chains = match &field {
        Some(_) => Some(heavy(ctx, CHAINS, q).await?),
        None => None,
    };
    let (c, venue) = (ctx.clone(), ctx.venue());
    tokio::task::spawn_blocking(move || {
        let map = match (&field, &chains) {
            (Some(f), Some(d)) => contracts(d, f)?,
            _ => Contracts::new(),
        };
        parse(&data, &bsc, &map, &venue, min_ok)
    })
    .await
    .map_err(|e| {
        c.tag(GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("разбор справочника сетей HTX прерван: {e}"),
        ))
    })?
}

/// Публичный GET с большим ответом. Ошибки: как rest::send.
async fn heavy(ctx: &Ctx, path: &str, q: Params) -> GatewayResult<serde_json::Value> {
    let r = Req {
        method: Method::GET,
        path,
        query: q,
        body: None,
        op: Op::Other,
        signed: false,
        heavy: true,
    };
    rest::send(ctx, r).await
}

/// Журнал: пропущенные сети, BSC с неизвестной комиссией, неоднозначные, нет поля адреса контракта (весь список —
/// только при изменении).
async fn note(ctx: &Ctx, nets: Option<&Networks>, p: &Parsed) {
    let mut lines = vec![];
    let mut add = |items: &[String], what: &str| {
        if !items.is_empty() {
            lines.push(format!("{what}: {}", items.join(", ")));
        }
    };
    add(&p.skipped, "пропущены сети с неразборчивыми полями");
    add(
        &p.fee_unknown,
        "сети BSC с комиссией вывода не fixed (circulated / ratio — как считает HTX, не описано: вывода нет)",
    );
    add(
        &p.ambiguous,
        "у монеты несколько сетей с именем сети BSC — сеть BSC не берётся",
    );
    if ctx.text("venue.contract_field").is_err() {
        lines.push(
            "адрес контракта BEP20 в документации HTX не определён, venue.contract_field не задана — монеты HTX не \
             сопоставляются (сделок нет)"
                .into(),
        );
    }
    let text = lines.join(" | ");
    if let Some(n) = nets {
        let mut t = n.told.write().unwrap_or_else(|e| e.into_inner());
        if *t == text {
            return;
        }
        *t = text.clone();
    }
    if !text.is_empty() {
        ctx.log(Level::Warn, "venue.networks", format!("HTX сети: {text}"))
            .await;
    }
}

/// Стоимость вывода: transactFeeWithdraw сети (fixed; иначе комиссия неизвестна).
/// Принимает: монету, сеть ядра. Ошибки: монеты или сети нет, комиссия неизвестна, связь.
pub async fn withdraw_cost(ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<Vec<Cost>> {
    let fee = withdraw_fee(ctx, &asset.ticker, network).await?;
    Ok(vec![Cost::new(
        CostKind::WithdrawFee,
        fee,
        asset.ticker.clone(),
        false,
    )])
}

/// Комиссия вывода монеты в сети ядра (свежий справочник одной монеты). Ошибки: сети нет, комиссия неизвестна.
pub async fn withdraw_fee(
    ctx: &Ctx,
    ticker: &str,
    network: &str,
) -> GatewayResult<rust_decimal::Decimal> {
    let list = coin_networks_of(ctx, ticker).await?;
    let info = list.iter().find(|c| c.network == network).ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("У {ticker} нет сети {network} в справочнике HTX"),
        ))
    })?;
    info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "HTX: комиссия вывода {ticker} в сети {network} неизвестна (withdrawFeeType не fixed или нет \
                 transactFeeWithdraw)"
            ),
        ))
    })
}
