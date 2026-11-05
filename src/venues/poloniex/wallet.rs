//! Файл: venues/poloniex/wallet.rs
//! Что делает: справочник монет и сетей Poloniex с кэшем в памяти не старше venue.networks_fresh_ms (решение 118):
//! весь список — GET /v2/currencies (ответ большой — разбор вне рабочих потоков, за `Arc` без копий, решение 131); одна
//! монета — GET /v2/currencies/{currency} (решение 113: свежий статус одной монеты для предпроверки); стоимость вывода.
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs (one_raw — имя монеты в сети).
//! От чего зависит: rest.rs (Req, heavy), networks_parse.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [poloniex] — имя сети BSC (поле blockchain; в документации и SDK примеры только BTC,
//! ETH, TRX — по живому ответу справочника, значения по умолчанию нет), venue.deposit_min_absent_ok [poloniex],
//! venue.networks_fresh_ms [poloniex] (необязательная).
//!
//! Официально: Currency Information V2 https://api-docs.poloniex.com/spot/api/public/reference-data — поля в шапке
//! networks_parse.rs; лимит запросов у /v2/currencies в таблице Rate Limits не заполнен. Справочник v1 (/currencies)
//! не используется: в нём нет адреса контракта (урок 8).

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

/// Путь справочника монет (Currency Information V2).
const PATH: &str = "/v2/currencies";

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    /// Последнее записанное в журнал (пропущенные сети; сети BSC с memo) — пишем только при изменении.
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

/// Все монеты Poloniex и их сети. Ошибки: настройки, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(Arc::unwrap_or_clone(shared(ctx, nets).await?))
}

/// Ответ /v2/currencies/{currency} как есть (одна монета). Ошибки: связь, коды Poloniex (неизвестная монета — код
/// ответа биржи; какой — в документации нет).
pub async fn one_raw(ctx: &Ctx, ticker: &str) -> GatewayResult<serde_json::Value> {
    let path = format!("{PATH}/{}", super::sign::encode(ticker));
    rest::public(ctx, &path, vec![]).await
}

/// Сети одной монеты — запросом /v2/currencies/{currency} (решение 113). Ошибки: как coin_networks.
pub async fn coin_networks_of(ctx: &Ctx, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let data = one_raw(ctx, ticker).await?;
    if data.is_null() {
        return Ok(vec![]);
    }
    let p = parse(&data, &bsc, &ctx.venue(), min_ok)?;
    note(ctx, None, &p).await;
    Ok(p.list
        .into_iter()
        .filter(|i| i.asset.ticker.eq_ignore_ascii_case(ticker))
        .collect())
}

/// Справочник за `Arc`: из кэша — указатель; запрос — разбор вне рабочих потоков. Ошибки: как coin_networks.
async fn shared(ctx: &Ctx, nets: &Networks) -> GatewayResult<Arc<Vec<CoinNetworkInfo>>> {
    let fresh = ctx
        .int("venue.networks_fresh_ms")
        .ok()
        .map(Duration::from_millis);
    if let Some(list) = fresh.and_then(|f| nets.cached(f)) {
        return Ok(list);
    }
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let r = Req {
        method: Method::GET,
        path: PATH,
        query: vec![],
        body: None,
        op: Op::Other,
        signed: false,
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
    Ok(list)
}

/// Журнал: пропущенные сети и сети BSC с memo (весь список — только при изменении).
async fn note(ctx: &Ctx, nets: Option<&Networks>, p: &Parsed) {
    let mut lines = vec![];
    let mut add = |items: &[String], what: &str| {
        if !items.is_empty() {
            lines.push(format!("{what}: {}", items.join(", ")));
        }
    };
    add(&p.skipped, "пропущены сети с неразборчивыми полями");
    add(
        &p.need_memo,
        "сети BSC с memo (currencyType = address-payment-id) — депозит не поддерживается",
    );
    let text = lines.join(" | ");
    if let Some(n) = nets {
        let mut t = n.told.write().unwrap_or_else(|e| e.into_inner());
        if *t == text {
            return;
        }
        *t = text.clone();
    }
    if !text.is_empty() {
        let m = format!("Poloniex /v2/currencies: {text}");
        ctx.log(Level::Warn, "venue.networks", m).await;
    }
}

/// Стоимость вывода: withdrawFee сети (в монетах).
/// Принимает: монету, сеть (blockchain). Ошибки: монеты или сети нет, комиссия неизвестна, связь.
pub async fn withdraw_cost(ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<Vec<Cost>> {
    let list = coin_networks_of(ctx, &asset.ticker).await?;
    let info = list.iter().find(|c| c.network == network).ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!(
                "У {} нет сети {network} в справочнике Poloniex",
                asset.ticker
            ),
        ))
    })?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "Poloniex: комиссия вывода {} в сети {network} неизвестна (нет withdrawFee)",
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
