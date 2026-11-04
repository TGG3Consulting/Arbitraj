//! Файл: venues/okx/wallet.rs
//! Что делает: справочник монет и сетей OKX с кэшем в памяти не старше venue.networks_fresh_ms (решение 118): весь
//! список — GET /api/v5/asset/currencies (ответ большой — разбор вне рабочих потоков, за `Arc` без копий, решение 131);
//! одна монета — тот же запрос с ccy= (решение 113: свежий статус одной монеты для предпроверки); chain OKX по сети ядра
//! (для вывода и адреса депозита); стоимость вывода.
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs, transfers.rs.
//! От чего зависит: rest.rs (Req, heavy), networks_parse.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [okx] — сеть BSC ядра (часть chain после «<монета>-», по живому ответу справочника;
//! значения по умолчанию нет), venue.deposit_min_absent_ok [okx] (нет minDep — считать 0), venue.networks_fresh_ms [okx]
//! (необязательная).
//!
//! Официально: «Funding Account → REST API → Get currencies» https://www.okx.com/docs-v5/en/ — «Retrieve a list of all
//! currencies available which are related to the current account's KYC entity», «6 requests per second» (User ID),
//! подпись обязательна («The API endpoints of Funding Account require authentication»), ccy — «Single currency or
//! multiple currencies separated with comma». Поля — в шапке networks_parse.rs.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Method;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::networks_parse::{Parsed, parse};
use super::rest::{self, Req};

/// Путь справочника монет.
const PATH: &str = "/api/v5/asset/currencies";

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    /// Последнее записанное в журнал (пропущенные; memo; адреса) — пишем только при изменении.
    told: Arc<RwLock<String>>,
}

impl Networks {
    /// Справочник из кэша, если он моложе fresh (указатель, без копии). Ошибок нет.
    fn cached(&self, fresh: Duration) -> Option<Arc<Vec<CoinNetworkInfo>>> {
        let c = self.cache.read().unwrap_or_else(|p| p.into_inner());
        c.as_ref()
            .filter(|(at, _)| at.elapsed() < fresh)
            .map(|(_, l)| l.clone())
    }
}

/// Все монеты OKX и их сети. Ошибки: настройки, ключ, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let fresh = ctx
        .int("venue.networks_fresh_ms")
        .ok()
        .map(Duration::from_millis);
    if let Some(list) = fresh.and_then(|f| nets.cached(f)) {
        return Ok(Arc::unwrap_or_clone(list));
    }
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let r = Req {
        method: Method::GET,
        path: PATH,
        query: vec![],
        body: None,
        op: Op::Other,
        signed: true,
        heavy: true,
    };
    let data = rest::send(ctx, r).await?;
    let (c, venue) = (ctx.clone(), ctx.venue());
    let p = tokio::task::spawn_blocking(move || parse(&data, &bsc, &venue, min_ok))
        .await
        .map_err(|e| {
            c.tag(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("разбор {PATH} прерван: {e}"),
            ))
        })??;
    note(ctx, Some(nets), &p).await;
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(Arc::unwrap_or_clone(list))
}

/// Одна монета — тот же запрос с ccy= (решение 113). Возвращает: разбор (сети и chain OKX). Ошибки: как coin_networks.
pub async fn one(ctx: &Ctx, ticker: &str) -> GatewayResult<Parsed> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let data = rest::get(ctx, PATH, vec![("ccy", ticker.to_string())]).await?;
    let p = parse(&data, &bsc, &ctx.venue(), min_ok)?;
    note(ctx, None, &p).await;
    Ok(p)
}

/// chain OKX для сети ядра монеты (например «BSC» → «CAKE-BSC» — как отдал справочник, без сборки строки).
/// Ошибки: NotConfigured — у монеты нет такой сети в справочнике OKX; как one.
pub async fn chain(
    ctx: &Ctx,
    ticker: &str,
    network: &str,
) -> GatewayResult<(String, CoinNetworkInfo)> {
    let p = one(ctx, ticker).await?;
    let chain = p
        .chains
        .iter()
        .find(|(t, n, _)| t == ticker && n == network)
        .map(|(_, _, c)| c.clone());
    let info = p.list.into_iter().find(|i| i.network == network);
    match (chain, info) {
        (Some(c), Some(i)) => Ok((c, i)),
        _ => Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("У {ticker} нет сети {network} в справочнике OKX (asset/currencies)"),
        ))),
    }
}

/// Журнал: пропущенные сети, сети BSC с memo и с неразборчивым адресом (весь список — только при изменении).
async fn note(ctx: &Ctx, nets: Option<&Networks>, p: &Parsed) {
    let mut lines = vec![];
    if !p.skipped.is_empty() {
        lines.push(format!(
            "пропущено сетей: {} — {}",
            p.skipped.len(),
            p.skipped.join("; ")
        ));
    }
    if !p.need_tag.is_empty() {
        lines.push(format!(
            "сети BSC с memo (needTag) — не поддерживаются: {}",
            p.need_tag.join(", ")
        ));
    }
    if !p.bad_contract.is_empty() {
        lines.push(format!(
            "сети BSC с ctAddr не полным адресом — монета не сопоставляется: {}",
            p.bad_contract.join(", ")
        ));
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
        ctx.log(
            Level::Warn,
            "venue.networks",
            format!("OKX currencies: {text}"),
        )
        .await;
    }
}

/// Стоимость вывода: fee сети (фиксированная, в монетах). Принимает: монету, сеть ядра.
/// Ошибки: монеты или сети нет, комиссия не пришла, связь.
pub async fn withdraw_cost(ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<Vec<Cost>> {
    let (_, info) = chain(ctx, &asset.ticker, network).await?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "OKX: комиссия вывода {} в сети {network} не пришла (fee)",
                asset.ticker
            ),
        ))
    })?;
    Ok(vec![Cost::new(
        CostKind::WithdrawFee,
        fee,
        asset.ticker.clone(),
        false,
    )])
}
