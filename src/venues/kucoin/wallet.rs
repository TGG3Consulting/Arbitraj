//! Файл: venues/kucoin/wallet.rs
//! Что делает: справочник монет и сетей KuCoin с кэшем в памяти не старше venue.networks_fresh_ms (решение 118):
//! весь список — GET /api/v3/currencies (ответ большой — разбор вне рабочих потоков, за `Arc` без копий, решение 131);
//! одна монета — GET /api/v3/currencies/{currency} (решение 113: свежий статус одной монеты для предпроверки);
//! стоимость вывода.
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs.
//! От чего зависит: rest.rs (Req, heavy), networks_parse.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [kucoin] — chainId сети BSC (по живому ответу справочника; значения по умолчанию нет),
//! venue.deposit_min_absent_ok [kucoin] (нет depositMinSize — считать 0), venue.networks_fresh_ms [kucoin] (необязательная).
//!
//! Официально: Get All Currencies (пул Public, вес 3) https://www.kucoin.com/docs-new/api-3470152, Get Currency
//! (Public, вес 3, параметр chain) https://www.kucoin.com/docs-new/api-3470155 — поля в шапке networks_parse.rs.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Method;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::networks_parse::{Parsed, parse};
use super::reply::Pool;
use super::rest::{self, Req};

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    /// Последнее записанное в журнал (пропущенные; memo; доля комиссии) — пишем только при изменении.
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

/// Все монеты KuCoin и их сети. Ошибки: настройки, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(Arc::unwrap_or_clone(shared(ctx, nets).await?))
}

/// Сети одной монеты — отдельным запросом Get Currency (решение 113). Ошибки: как coin_networks; монеты нет — пусто.
pub async fn coin_networks_of(ctx: &Ctx, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let path = format!("/api/v3/currencies/{ticker}");
    let data = rest::public(ctx, &path, vec![], Pool::Public).await?;
    if data.is_null() {
        return Ok(vec![]);
    }
    let p = parse(&data, &bsc, &ctx.venue(), min_ok)?;
    note(ctx, None, &p).await;
    Ok(p.list)
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
        path: "/api/v3/currencies",
        query: vec![],
        body: None,
        op: Op::Other,
        pool: Pool::Public,
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
                format!("разбор /api/v3/currencies прерван: {e}"),
            ))
        })??;
    note(ctx, Some(nets), &p).await;
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(list)
}

/// Журнал: пропущенные сети, сети BSC с memo и с долей комиссии вывода (весь список — только при изменении).
async fn note(ctx: &Ctx, nets: Option<&Networks>, p: &Parsed) {
    let mut lines = vec![];
    if !p.skipped.is_empty() {
        lines.push(format!(
            "пропущено сетей с неразборчивыми полями: {} — {}",
            p.skipped.len(),
            p.skipped.join("; ")
        ));
    }
    if !p.need_tag.is_empty() {
        lines.push(format!(
            "сети BSC с memo (needTag) — депозит не поддерживается: {}",
            p.need_tag.join(", ")
        ));
    }
    if !p.fee_rate.is_empty() {
        lines.push(format!(
            "сети BSC с ненулевой withdrawFeeRate (сырые значения; формата и формулы в документации нет — комиссия \
             неизвестна, вывода нет; решение 199): {}",
            p.fee_rate.join(", ")
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
            format!("KuCoin currencies: {text}"),
        )
        .await;
    }
}

/// Стоимость вывода: withdrawalMinFee сети (в монетах; сети с долей комиссии — комиссия неизвестна).
/// Принимает: монету, сеть (chainId). Ошибки: монеты или сети нет, комиссия неизвестна, связь.
pub async fn withdraw_cost(ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<Vec<Cost>> {
    let list = coin_networks_of(ctx, &asset.ticker).await?;
    let info = list.iter().find(|c| c.network == network).ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("У {} нет сети {network} в справочнике KuCoin", asset.ticker),
        ))
    })?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "KuCoin: комиссия вывода {} в сети {network} неизвестна (нет withdrawalMinFee или ненулевой withdrawFeeRate)",
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
