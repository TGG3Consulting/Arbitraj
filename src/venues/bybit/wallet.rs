//! Файл: venues/bybit/wallet.rs
//! Что делает: справочник монет и сетей Bybit (coin/query-info) с кэшем в памяти не старше venue.networks_fresh_ms;
//! справочник одной монеты (тот же запрос с coin=, решение 113 — свежий статус для предпроверки); стоимость вывода.
//! Решение 131: справочник — за Arc (кэш без копии), ответ и разбор — вне рабочих потоков tokio.
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs.
//! От чего зависит: rest.rs, networks_parse.rs (разбор), ctx.rs, core/types.
//! Настройки: venue.bsc_network [bybit] — название сети BSC у Bybit (поле chain; ядро сравнивает с ним сеть монеты),
//! venue.networks_fresh_ms [bybit] (необязательная; не задана — запрос каждый раз).
//!
//! Официальная документация: GET /v5/asset/coin/query-info (подписанный; параметр coin — «Coin, uppercase only»,
//! необязательный; лимит «5 req/s») https://bybit-exchange.github.io/docs/v5/asset/coin-info — поля в шапке
//! networks_parse.rs.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::errors::Op;
use super::networks_parse::{Parsed, parse_networks};
use super::rest;

/// Путь справочника монет.
const PATH: &str = "/v5/asset/coin/query-info";

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    /// Последний записанный в журнал список предупреждений — пишем только при изменении.
    told: Arc<RwLock<String>>,
    cache: Arc<RwLock<Option<Cached>>>,
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

/// Все монеты Bybit и их сети. Ошибки: настройки, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(Arc::unwrap_or_clone(shared(ctx, nets).await?))
}

/// Сети одной монеты — запросом с coin= (решение 113): свежий статус, без всего справочника. Ошибки: как coin_networks.
pub async fn coin_networks_of(ctx: &Ctx, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let bsc = ctx.text("venue.bsc_network")?;
    let data = rest::get(ctx, PATH, vec![("coin", ticker.to_string())], Op::Other).await?;
    // Предупреждения разбора пишет полный справочник (shared): одна монета их не перезаписывает.
    let p = parse_networks(&data, &bsc, &ctx.venue())?;
    Ok(p.list
        .into_iter()
        .filter(|i| i.asset.ticker == ticker)
        .collect())
}

/// Справочник за Arc: из кэша — указатель; запрос — ответ и разбор вне рабочих потоков tokio.
async fn shared(ctx: &Ctx, nets: &Networks) -> GatewayResult<Arc<Vec<CoinNetworkInfo>>> {
    let fresh = ctx
        .int("venue.networks_fresh_ms")
        .ok()
        .map(Duration::from_millis);
    if let Some(list) = fresh.and_then(|f| nets.cached(f)) {
        return Ok(list);
    }
    let bsc = ctx.text("venue.bsc_network")?;
    let data = rest::get_heavy(ctx, PATH, vec![], Op::Other).await?;
    let venue = ctx.venue();
    let p = tokio::task::spawn_blocking(move || parse_networks(&data, &bsc, &venue))
        .await
        .map_err(|e| {
            ctx.tag(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("разбор coin/query-info прерван: {e}"),
            ))
        })??;
    warn(ctx, nets, &p).await;
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(list)
}

/// Предупреждения разбора (пропущенные сети, процентная комиссия у BSC) — в журнал, только если изменились.
async fn warn(ctx: &Ctx, nets: &Networks, p: &Parsed) {
    let mut lines = vec![];
    if !p.skipped.is_empty() {
        lines.push(format!(
            "coin/query-info: пропущено сетей с неразборчивыми полями: {} — {}",
            p.skipped.len(),
            p.skipped.join("; ")
        ));
    }
    if !p.percent_fee.is_empty() {
        lines.push(format!(
            "coin/query-info: у сетей BSC процентная комиссия вывода (withdrawPercentageFee) — {}; ядро считает \
             комиссию числом монет, поэтому комиссия вывода этих монет «неизвестна» и сделок с выводом по ним нет",
            p.percent_fee.join(", ")
        ));
    }
    let text = lines.join("\n");
    {
        let mut t = nets.told.write().unwrap_or_else(|e| e.into_inner());
        if *t == text {
            return;
        }
        *t = text.clone();
    }
    for m in lines {
        let _ = ctx
            .journal
            .write(Level::Warn, "venue.networks", Some(ID), m, None)
            .await;
    }
}

/// Стоимость вывода: withdrawFee из coin/query-info (в монетах). Принимает: монету, сеть (chain).
/// Возвращает: [WithdrawFee]. Ошибки: монеты или сети нет, комиссия не сообщена (или процентная), связь.
pub async fn withdraw_cost(
    ctx: &Ctx,
    nets: &Networks,
    asset: &Asset,
    network: &str,
) -> GatewayResult<Vec<Cost>> {
    let all = shared(ctx, nets).await?;
    let info = all
        .iter()
        .find(|c| c.asset.ticker == asset.ticker && c.network == network)
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!("У {} нет сети {network} в справочнике Bybit", asset.ticker),
            ))
        })?;
    let fee: Decimal = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "Bybit не сообщил комиссию вывода {} в сети {network} числом (пусто или процентная)",
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
