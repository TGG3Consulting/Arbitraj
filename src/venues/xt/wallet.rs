//! Файл: venues/xt/wallet.rs
//! Что делает: справочник монет и сетей XT.com (support/currency + статусы монет currencies) с кэшем в памяти не
//! старше venue.networks_fresh_ms и стоимость вывода. Справочник — за `Arc` (без копий), ответ и разбор — вне рабочих
//! потоков tokio (решение 131).
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs.
//! От чего зависит: rest.rs, networks_parse.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [xt] (название сети BSC в поле chain — его принимает вывод и адрес депозита; в
//! документации не указано — из живого справочника), venue.deposit_min_absent_ok [xt] (минимума депозита в API XT нет —
//! решение человека, урок 21), venue.networks_fresh_ms [xt] (необязательная; не задана — запрос каждый раз).
//!
//! Официально: GET /v4/public/wallet/support/currency и GET /v4/public/currencies (публичные, без параметров) — весь
//! список одним ответом: запроса одной монеты нет, coin_networks_of — фильтр общего справочника (решение 113).

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::networks_parse::{parse_networks, parse_statuses};
use super::rest;

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    /// Последний записанный в журнал список пропущенных сетей — пишем только при изменении.
    told: Arc<RwLock<String>>,
}

/// Все монеты и сети. Ошибки: настройки, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(Arc::unwrap_or_clone(shared(ctx, nets).await?))
}

/// Сети одной монеты (фильтр общего справочника без копии всех монет). Ошибки: как coin_networks.
pub async fn coin_networks_of(
    ctx: &Ctx,
    nets: &Networks,
    ticker: &str,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let all = shared(ctx, nets).await?;
    Ok(all
        .iter()
        .filter(|i| i.asset.ticker == ticker)
        .cloned()
        .collect())
}

/// Справочник за `Arc`: из кэша — указатель; иначе запрос и разбор в spawn_blocking. Ошибки: как coin_networks.
pub async fn shared(ctx: &Ctx, nets: &Networks) -> GatewayResult<Arc<Vec<CoinNetworkInfo>>> {
    let fresh = ctx
        .int("venue.networks_fresh_ms")
        .ok()
        .map(Duration::from_millis);
    if let Some(f) = fresh {
        let c = nets.cache.read().unwrap_or_else(|p| p.into_inner());
        if let Some((at, list)) = c.as_ref()
            && at.elapsed() < f
        {
            return Ok(list.clone());
        }
    }
    let bsc = ctx.text("venue.bsc_network")?;
    let absent_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let (support, statuses) = tokio::join!(
        rest::public_heavy(ctx, "/v4/public/wallet/support/currency", vec![]),
        rest::public_heavy(ctx, "/v4/public/currencies", vec![]),
    );
    let (support, statuses) = (support?, statuses?);
    let (c, venue) = (ctx.clone(), ctx.venue());
    let p = tokio::task::spawn_blocking(move || {
        let st = parse_statuses(&statuses)?;
        parse_networks(&support, &st, &bsc, &venue, absent_ok)
    })
    .await
    .map_err(|e| {
        c.tag(GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("разбор справочника сетей XT прерван: {e}"),
        ))
    })??;
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    let skipped = p.skipped.join("; ");
    let new = {
        let mut t = nets.told.write().unwrap_or_else(|e| e.into_inner());
        let changed = *t != skipped;
        *t = skipped.clone();
        changed
    };
    if new && !p.skipped.is_empty() {
        let m = format!(
            "Справочник сетей XT: пропущено или ограничено сетей: {} — {skipped}",
            p.skipped.len()
        );
        ctx.log(Level::Warn, "venue.networks", m).await;
    }
    Ok(list)
}

/// Сеть монеты из справочника. Ошибка NotConfigured — монеты или сети нет.
pub async fn network(
    ctx: &Ctx,
    nets: &Networks,
    ticker: &str,
    network: &str,
) -> GatewayResult<CoinNetworkInfo> {
    let all = shared(ctx, nets).await?;
    all.iter()
        .find(|c| c.asset.ticker == ticker && c.network == network)
        .cloned()
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!("У {ticker} нет сети {network} в справочнике XT"),
            ))
        })
}

/// Стоимость вывода: withdrawFeeAmount справочника (в монетах; сумма вывода включает комиссию — transfers.rs). Ошибки: монеты или сети нет, комиссия не сообщена, связь.
pub async fn withdraw_cost(
    ctx: &Ctx,
    nets: &Networks,
    asset: &Asset,
    net: &str,
) -> GatewayResult<Vec<Cost>> {
    let info = network(ctx, nets, &asset.ticker, net).await?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "XT не сообщил комиссию вывода {} в сети {net}",
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
