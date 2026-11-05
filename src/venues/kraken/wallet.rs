//! Файл: venues/kraken/wallet.rs
//! Что делает: справочник монет и сетей Kraken с кэшем в памяти не старше venue.networks_fresh_ms (решение 118): весь —
//! методы ввода и вывода Funding (Beta) + активы спота (разбор вне рабочих потоков, за `Arc`, решение 131); одна монета —
//! те же запросы с фильтром монеты (решение 113); method_id по монете, сети и направлению (вывод, адрес депозита);
//! стоимость вывода.
//! Кто вызывает: mod.rs (функции контракта), deposit_addr.rs, transfers.rs.
//! От чего зависит: methods.rs, networks_parse.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [kraken] — network_name сети BSC в методах Funding (по живому ответу; значения по
//! умолчанию нет), venue.deposit_min_absent_ok [kraken] (нет minimum_amount — считать 0), venue.networks_fresh_ms [kraken]
//! (необязательная).
//!
//! Официально — https://docs.kraken.com/api-reference/funding-beta/list-funding-methods.md и OpenAPI (Assets); поля —
//! в шапках methods.rs и networks_parse.rs. Сети BSC в документации нет ни в одном примере — её имя только из живого
//! ответа (docs/notes/kraken-analysis-2026-11-05.md, вердикт).

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::methods::{self, Dir};
use super::networks_parse::{Parsed, parse};

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    /// Последнее записанное в журнал — пишем только при изменении.
    told: Arc<RwLock<String>>,
}

/// Все монеты Kraken и их сети. Ошибки: настройки, ключ, права, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let fresh = ctx
        .int("venue.networks_fresh_ms")
        .ok()
        .map(Duration::from_millis);
    if let Some(f) = fresh {
        let c = nets.cache.read().unwrap_or_else(|p| p.into_inner());
        if let Some((_, l)) = c.as_ref().filter(|(at, _)| at.elapsed() < f) {
            return Ok(l.as_ref().clone());
        }
    }
    let p = load(ctx, None).await?;
    note(ctx, Some(nets), &p).await;
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(Arc::unwrap_or_clone(list))
}

/// Одна монета (решение 113). Возвращает: разбор (сети и методы). Ошибки: как coin_networks.
pub async fn one(ctx: &Ctx, ticker: &str) -> GatewayResult<Parsed> {
    let p = load(ctx, Some(ticker)).await?;
    note(ctx, None, &p).await;
    Ok(p)
}

/// Чтение и разбор (вне рабочих потоков). Принимает: монету (None — все). Ошибки: настройки, запросы.
async fn load(ctx: &Ctx, ticker: Option<&str>) -> GatewayResult<Parsed> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let dep = methods::list(ctx, Dir::Deposit, ticker).await?;
    let wd = methods::list(ctx, Dir::Withdraw, ticker).await?;
    let assets = methods::assets(ctx, ticker).await?;
    let (c, venue) = (ctx.clone(), ctx.venue());
    tokio::task::spawn_blocking(move || parse(&dep, &wd, &assets, &bsc, &venue, min_ok))
        .await
        .map_err(|e| {
            c.tag(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("разбор методов Funding прерван: {e}"),
            ))
        })?
}

/// method_id и сеть монеты по сети ядра и направлению. Ошибки: NotConfigured — у монеты нет такого метода; как one.
pub async fn method(
    ctx: &Ctx,
    ticker: &str,
    network: &str,
    dir: Dir,
) -> GatewayResult<(String, CoinNetworkInfo)> {
    let p = one(ctx, ticker).await?;
    let id = p
        .rows
        .iter()
        .find(|r| r.ticker == ticker && r.network == network && r.dir == dir)
        .map(|r| r.method_id.clone());
    let info = p
        .list
        .into_iter()
        .find(|i| i.asset.ticker == ticker && i.network == network);
    match (id, info) {
        (Some(id), Some(i)) => Ok((id, i)),
        _ => Err(ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!(
                "У {ticker} нет метода {} в сети {network} в справочнике Kraken (funding/v1/methods)",
                dir.path()
            ),
        ))),
    }
}

/// Журнал: замечания разбора (весь список — только при изменении).
async fn note(ctx: &Ctx, nets: Option<&Networks>, p: &Parsed) {
    let text = p.notes.join("; ");
    if let Some(n) = nets {
        let mut t = n.told.write().unwrap_or_else(|e| e.into_inner());
        if *t == text {
            return;
        }
        *t = text.clone();
    }
    if !text.is_empty() {
        let m = format!("Kraken методы Funding: {text}");
        ctx.log(Level::Warn, "venue.networks", m).await;
    }
}

/// Стоимость вывода: фиксированная комиссия метода (fees.base). Принимает: монету, сеть ядра.
/// Ошибки: монеты или метода нет; комиссия процентная или не пришла (BadResponse); связь.
pub async fn withdraw_cost(ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<Vec<Cost>> {
    let (_, info) = method(ctx, &asset.ticker, network, Dir::Withdraw).await?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "Kraken: комиссия вывода {} в сети {network} неизвестна (процентная или не пришла)",
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
