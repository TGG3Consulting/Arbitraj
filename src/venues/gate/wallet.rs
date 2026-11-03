//! Файл: venues/gate/wallet.rs
//! Что делает: справочник монет и сетей Gate с кэшем не старше venue.networks_fresh_ms (решения 118, 131: справочник
//! за `Arc`, ответы и разбор — вне рабочих потоков tokio), сети одной монеты свежими запросами по монете (решение 113),
//! (стоимость вывода — wallet_cost.rs). Помнит сведения монет, которых нет в общих справочниках (точность вывода, минимум депозита,
//! подтверждения — networks_parse::Extra).
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs (сведения адреса депозита).
//! От чего зависит: rest.rs, networks_parse.rs, ctx.rs, symbols.rs (запасная точность), core/types.
//! Настройки: venue.bsc_network [gate] — название сети BSC у Gate (поле name сети / chain; ядро сравнивает с ним сеть
//! монеты), venue.deposit_min_absent_ok [gate], venue.networks_fresh_ms [gate] (необязательная).
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ : GET /spot/currencies (публичный, все
//! монеты), GET /spot/currencies/{currency} («Query single currency information»), GET /wallet/withdraw_status
//! (приватный, «optionally filtered by currency»), GET /wallet/currency_chains?currency= (публичный, одна монета; «API
//! operations are not supported for tokens with low liquidity or extremely low value») — поля в шапке networks_parse.rs.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::Value;

use crate::core::types::CoinNetworkInfo;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::networks_parse::{Extra, Parsed, chains_extra, parse_networks};
use super::rest;
use super::symbols::Symbols;

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    extra: Arc<RwLock<HashMap<String, Extra>>>,
    /// Последний записанный в журнал список «неясной» комиссии — пишем только при изменении.
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

    /// Сведения адреса депозита монеты (deposit_addr.rs): минимум депозита и подтверждения сети. Ошибок нет.
    pub fn note_deposit(
        &self,
        ticker: &str,
        min: Option<rust_decimal::Decimal>,
        chain: &str,
        confirms: Option<u32>,
    ) {
        let mut m = self.extra.write().unwrap_or_else(|p| p.into_inner());
        let e = m.entry(ticker.to_string()).or_default();
        if min.is_some() {
            e.deposit_min = min;
        }
        if let Some(c) = confirms {
            e.confirms.insert(chain.to_string(), c);
        }
    }
}

/// Тикер годится в путь URL (/spot/currencies/{currency}) без кодирования. Ошибка BadResponse — нет.
pub fn path_safe(ctx: &Ctx, ticker: &str) -> GatewayResult<()> {
    if !ticker.is_empty()
        && ticker
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Ok(());
    }
    let m = format!("Тикер «{ticker}» нельзя передать в пути запроса Gate");
    Err(ctx.tag(GatewayError::new(ErrorCode::BadResponse, None, m)))
}

/// Справочник: все монеты Gate и их сети. Ошибки: настройки, связь, формат.
pub async fn coin_networks(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(Arc::unwrap_or_clone(shared(ctx, nets, symbols).await?))
}

/// Сети одной монеты — свежими запросами по монете (решение 113): /spot/currencies/{c}, /wallet/withdraw_status?currency=,
/// /wallet/currency_chains?currency= (сбой последнего — без точности сети, в журнал). Ошибки: как coin_networks.
pub async fn coin_networks_of(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
    ticker: &str,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    path_safe(ctx, ticker)?;
    let cur = rest::public(ctx, &format!("/spot/currencies/{ticker}"), vec![]).await?;
    let p = vec![("currency", ticker.to_string())];
    let w = rest::signed(
        ctx,
        Method::GET,
        "/wallet/withdraw_status",
        p.clone(),
        None,
        Op::Other,
    )
    .await?;
    match rest::public(ctx, "/wallet/currency_chains", p).await {
        Ok(ch) => {
            let old = nets
                .extra
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(ticker)
                .cloned();
            let (ex, _) = chains_extra(&ch, old.unwrap_or_default())?;
            nets.extra
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(ticker.to_string(), ex);
        }
        Err(e) => {
            log(
                ctx,
                format!(
                    "/wallet/currency_chains {ticker}: {} — точность вывода по паре",
                    e.message
                ),
            )
            .await
        }
    }
    Ok(parse(ctx, nets, symbols, &cur, &w)?.list)
}

/// Справочник за `Arc`: из кэша — указатель; иначе два запроса (разбор — вне рабочих потоков). Ошибки: как выше.
pub(super) async fn shared(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
) -> GatewayResult<Arc<Vec<CoinNetworkInfo>>> {
    let fresh = ctx
        .int("venue.networks_fresh_ms")
        .ok()
        .map(Duration::from_millis);
    if let Some(list) = fresh.and_then(|f| nets.cached(f)) {
        return Ok(list);
    }
    symbols.ensure_fresh(ctx).await?;
    let cur = rest::public_heavy(ctx, "/spot/currencies", vec![]).await?;
    let w = rest::signed_heavy(ctx, "/wallet/withdraw_status", vec![]).await?;
    let (c, n, s) = (ctx.clone(), nets.clone(), symbols.clone());
    let p = tokio::task::spawn_blocking(move || parse(&c, &n, &s, &cur, &w))
        .await
        .map_err(|e| {
            ctx.tag(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("разбор справочника прерван: {e}"),
            ))
        })??;
    let unclear = p.unclear_fee.join(", ");
    let new = {
        let mut t = nets.told.write().unwrap_or_else(|e| e.into_inner());
        let r = *t != unclear;
        *t = unclear.clone();
        r
    };
    if new && !unclear.is_empty() {
        let m = format!(
            "withdraw_percent_on_chains не ноль в сети BSC ({unclear}): формат процента не описан — комиссия вывода неизвестна, по этим монетам сделок нет"
        );
        log(ctx, m).await;
    }
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(list)
}

/// Разбор с настройками и сведениями монет. Ошибки: NotConfigured (venue.bsc_network), формат.
fn parse(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
    cur: &Value,
    w: &Value,
) -> GatewayResult<Parsed> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let extra = nets.extra.read().unwrap_or_else(|e| e.into_inner());
    parse_networks(cur, w, &bsc, &ctx.venue(), min_ok, &extra, &|t| {
        symbols.precision_of(t)
    })
}

/// Предупреждение в журнал (venue.networks).
async fn log(ctx: &Ctx, msg: String) {
    let _ = ctx
        .journal
        .write(
            Level::Warn,
            "venue.networks",
            Some(super::ctx::ID),
            msg,
            None,
        )
        .await;
}
