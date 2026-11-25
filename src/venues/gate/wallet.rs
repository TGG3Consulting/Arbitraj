//! Файл: venues/gate/wallet.rs
//! Что делает: справочник монет и сетей Gate с кэшем не старше venue.networks_fresh_ms (решения 118, 131: справочник
//! за `Arc`, ответы и разбор — вне рабочих потоков tokio), сети одной монеты свежими запросами по монете (решение 113),
//! (стоимость вывода — wallet_cost.rs). Помнит сведения монет, которых нет в общих справочниках (точность вывода, минимум депозита,
//! подтверждения — networks_parse::Extra). Решение 387: новый минимум депозита сразу попадает и в собранный справочник
//! (кэш); удачное получение справочника — толчок фоновому кругу сведений депозита (deposit_min.rs, deposit_info.rs).
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs (сведения адреса депозита).
//! От чего зависит: rest.rs, networks_parse.rs, ctx.rs, symbols.rs (запасная точность), core/types.
//! Настройки (решение 431: и venue.arbitrum_network [gate] — свои сети, gateway/own_nets.rs): venue.bsc_network [gate] — название сети BSC у Gate (поле name сети / chain; ядро сравнивает с ним сеть
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
    pub(super) cache: Arc<RwLock<Option<Cached>>>,
    pub(super) extra: Arc<RwLock<HashMap<String, Extra>>>,
    /// Решение 387: сколько раз справочник получен удачно и толчок об этом — фоновый круг сведений депозита
    /// (deposit_info.rs) после сбоя (секреты, связь) повторяет запросы сразу, как справочник снова получен.
    pub(super) fetched: Arc<std::sync::atomic::AtomicU64>,
    pub(super) fetched_note: Arc<tokio::sync::Notify>,
    /// Решение 387 (ревью Б1): монета → метка справочника (fetched), при которой Gate отказал в адресе депозита не по
    /// временной причине («deposit disabled»): до нового справочника адрес по запросу ядра не спрашивается.
    pub(super) refused: Arc<RwLock<HashMap<String, u64>>>,
    /// Решение 387 (ревью Н3): время последнего запроса адреса по запросу ядра — темп venue.deposit_info_requests_per_sec.
    pub(super) pace: Arc<tokio::sync::Mutex<Option<tokio::time::Instant>>>,
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

    pub(super) fn cached_tariff(&self, ticker: &str, network: &str) -> Option<CoinNetworkInfo> {
        self.cache
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()?
            .1
            .iter()
            .find(|r| r.asset.ticker == ticker && r.network == network)
            .cloned()
    }

    pub(super) fn refresh_tariff(&self, old: Option<CoinNetworkInfo>, new: &CoinNetworkInfo) {
        let Some(old) = old else { return };
        if old.asset != new.asset
            || old.withdraw_fee == new.withdraw_fee && old.withdraw_fee_pct == new.withdraw_fee_pct
        {
            return;
        }
        let mut cached = self.cache.write().unwrap_or_else(|p| p.into_inner());
        if let Some((_, list)) = cached.as_mut()
            && let Some(i) = list.iter().position(|r| {
                r.asset.ticker == old.asset.ticker
                    && r.asset == old.asset
                    && r.network == old.network
                    && r.withdraw_fee == old.withdraw_fee
                    && r.withdraw_fee_pct == old.withdraw_fee_pct
            })
        {
            let row = &mut Arc::make_mut(list)[i];
            row.withdraw_fee = new.withdraw_fee;
            row.withdraw_fee_pct = new.withdraw_fee_pct;
        }
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
        let e = e.clone();
        drop(m);
        // Решение 387: и в уже собранный справочник (кэш) — иначе ядро до конца окна venue.networks_fresh_ms получало
        // бы прежний справочник без минимума и затирало им минимум, полученный запросом по монете.
        let mut c = self.cache.write().unwrap_or_else(|p| p.into_inner());
        let stale = |x: &&mut Cached| super::deposit_min::stale(x.1.as_slice(), ticker, &e);
        if let Some((_, list)) = c.as_mut().filter(stale) {
            // make_mut — по Vec (с ожидаемым срезом вывод типа взял бы Arc<[…]>).
            let all: &mut Vec<CoinNetworkInfo> = Arc::make_mut(list);
            super::deposit_min::apply(all, ticker, &e);
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
            // Решение 387: прочитать и записать под одной блокировкой — минимум депозита, записанный одновременным
            // запросом адреса депозита (deposit_min.rs for_deposit), не затирается прежними сведениями.
            let mut m = nets.extra.write().unwrap_or_else(|e| e.into_inner());
            let old = m.get(ticker).cloned().unwrap_or_default();
            let (ex, _) = chains_extra(&ch, old)?;
            m.insert(ticker.to_string(), ex);
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
            "withdraw_percent_on_chains в сети BSC без знака «%» (сырые значения: {unclear}): формат не описан — комиссия вывода неизвестна, по этим монетам сделок нет (решение 199: «X%» учитывается как доля X/100)"
        );
        log(ctx, m).await;
    }
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    nets.mark_fetched();
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
    let bsc = crate::gateway::own_nets::read(&ctx.settings, ctx.at()).map_err(|e| ctx.tag(e))?;
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
