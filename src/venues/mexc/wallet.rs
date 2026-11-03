//! Файл: venues/mexc/wallet.rs
//! Что делает: справочник монет и сетей MEXC (config/getall) с кэшем в памяти не старше venue.networks_fresh_ms
//! (решение 118, А-🟡13: ответ ~5.6 МБ, вес 10, фильтра по монете нет), стоимость вывода. Помнит соответствие кода сети
//! (netWork — для нового вывода) и отображаемого имени (network — для адреса депозита и истории).
//! Решение 131: справочник — за `Arc` (кэш без копии), ответ и разбор — вне рабочих потоков tokio; сети одной монеты
//! (coin_networks_of) — фильтром общего справочника без копии всех монет.
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs (имя сети).
//! От чего зависит: rest.rs, networks_parse.rs (разбор), ctx.rs, symbols.rs (точность), core/types.
//! Настройки: venue.bsc_network [mexc] — код netWork сети BSC (тот, что принимает новый вывод; ядро сравнивает с ним
//! название сети монеты), venue.deposit_min_absent_ok [mexc] (решение 107: минимального депозита в API MEXC нет),
//! venue.networks_fresh_ms [mexc] (необязательная; не задана — запрос каждый раз).
//!
//! Официальная документация: GET /api/v3/capital/config/getall (SPOT_WITHDRAW_READ, вес 10, «Request parameters:
//! None») https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/query-the-currency-information — поля в шапке
//! networks_parse.rs.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Method;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::networks_parse::parse_networks;
pub use super::networks_parse::{flag, max_limit};
use super::rest;
use super::symbols::Symbols;

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    names: Arc<RwLock<HashMap<(String, String), String>>>,
    /// Последние записанные в журнал списки (пропущенные сети; сети BSC с неясным шагом) — пишем только при изменении.
    told: Arc<RwLock<(String, String)>>,
    /// Последний справочник и когда он получен (кэш venue.networks_fresh_ms).
    cache: Arc<RwLock<Option<Cached>>>,
}

impl Networks {
    /// Отображаемое имя сети монеты по коду. None — справочник ещё не получен или такой сети нет. Ошибок нет.
    pub fn display(&self, ticker: &str, code: &str) -> Option<String> {
        let m = self.names.read().unwrap_or_else(|p| p.into_inner());
        m.get(&(ticker.to_string(), code.to_string())).cloned()
    }

    /// Справочник из кэша, если он моложе fresh (указатель, без копии — решение 131). Ошибок нет.
    fn cached(&self, fresh: Duration) -> Option<Arc<Vec<CoinNetworkInfo>>> {
        let c = self.cache.read().unwrap_or_else(|p| p.into_inner());
        c.as_ref()
            .filter(|(at, _)| at.elapsed() < fresh)
            .map(|(_, l)| l.clone())
    }
}

/// Справочник: все монеты MEXC и их сети. Название сети ядра — код netWork (нет его — отображаемое имя); адрес
/// контракта — только у сети BSC (venue.bsc_network совпал с кодом netWork). Точность вывода — знаки пары монеты
/// (baseAssetPrecision) и котируемой монеты (quoteAssetPrecision), решение 107. Задана venue.networks_fresh_ms —
/// справочник моложе неё берётся из памяти без запроса. Ошибки: настройки, связь, формат.
pub async fn coin_networks(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(Arc::unwrap_or_clone(shared(ctx, nets, symbols).await?))
}

/// Сети одной монеты (решение 131: из общего справочника без копии всех монет — предпроверка сделки).
/// Ошибки: как coin_networks.
pub async fn coin_networks_of(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
    ticker: &str,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let all = shared(ctx, nets, symbols).await?;
    Ok(all
        .iter()
        .filter(|i| i.asset.ticker == ticker)
        .cloned()
        .collect())
}

/// Справочник за `Arc` (решение 131): из кэша — указатель; запрос — ответ разбирается вне рабочих потоков tokio
/// (rest::signed_heavy, spawn_blocking для networks_parse). Ошибки: как coin_networks.
async fn shared(
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
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    symbols.ensure_fresh(ctx).await?;
    let data = rest::signed_heavy(
        ctx,
        Method::GET,
        "/api/v3/capital/config/getall",
        vec![],
        Op::Other,
    )
    .await?;
    let (c, sy, venue) = (ctx.clone(), symbols.clone(), ctx.venue());
    let p = tokio::task::spawn_blocking(move || parse_networks(&data, &bsc, &venue, min_ok, &sy))
        .await
        .map_err(|e| {
            c.tag(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("разбор config/getall прерван: {e}"),
            ))
        })??;
    let list = Arc::new(p.list);
    *nets.names.write().unwrap_or_else(|e| e.into_inner()) = p.names;
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    let (skipped, unclear) = (p.skipped.join("; "), p.unclear_step.join(", "));
    let (new_skipped, new_unclear) = {
        let mut t = nets.told.write().unwrap_or_else(|e| e.into_inner());
        let r = (t.0 != skipped, t.1 != unclear);
        *t = (skipped.clone(), unclear.clone());
        r
    };
    if new_skipped && !p.skipped.is_empty() {
        // Сеть с неразборчивым полем не попадает в справочник (по ней нет сделок), остальные — работают.
        let msg = format!(
            "config/getall: пропущено сетей с неразборчивыми полями: {} — {skipped}",
            p.skipped.len()
        );
        log(ctx, msg).await;
    }
    if new_unclear && !p.unclear_step.is_empty() {
        let msg = format!(
            "config/getall: у {} сетей BSC withdrawIntegerMultiple — целое ({unclear}); в документации «Withdraw amount \
             increment», в примере — \"18\": смысл не подтверждён, шаг вывода не применяется (решение 118, А-🟠7)",
            p.unclear_step.len()
        );
        log(ctx, msg).await;
    }
    Ok(list)
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

/// Стоимость вывода: withdrawFee из getall (в монетах). Принимает: монету, сеть (код), сумму (не влияет).
/// Возвращает: [WithdrawFee]. Ошибки: монеты или сети нет, комиссия не сообщена, связь.
pub async fn withdraw_cost(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
    asset: &Asset,
    network: &str,
) -> GatewayResult<Vec<Cost>> {
    let all = shared(ctx, nets, symbols).await?;
    let info = all
        .iter()
        .find(|c| c.asset.ticker == asset.ticker && c.network == network)
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!("У {} нет сети {network} в справочнике MEXC", asset.ticker),
            ))
        })?;
    let fee: Decimal = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "MEXC не сообщил комиссию вывода {} в сети {network}",
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
