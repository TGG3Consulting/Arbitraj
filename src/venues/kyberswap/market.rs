//! Файл: venues/kyberswap/market.rs
//! Что делает: «Живой экран» для DEX через агрегатор. У агрегатора нет потока цен, поэтому цены берутся ПОЛЛИНГОМ:
//! раз в kyber.quote_poll_ms по активным монетам запрашивается маршрут USDT→монета на пробный объём kyber.probe_usdt,
//! и ядру шлётся событие MarketEvent::Pool — маркер присутствия и цены DEX, по которому ядро допускает направление
//! BingX ↔ Kyber и пересчитывает спред (ядро цену берёт из quote(), а не из полей пула — pool-math ядро не считает).
//! Пропал маршрут монеты — MarketEvent::Invalidated. При kyber.assume_zero_transfer_tax новым монетам шлётся
//! MarketEvent::CoinCheck с нулевыми налогами (налог свопа уже в котировке; налог перевода принят нулевым человеком),
//! чтобы на экране показывались цены, а не «налоги не проверены».
//! ВАЖНО (решение 91): поля резервов в Pool отражают пробный маршрут агрегатора (реальные вход/выход), а НЕ чтение
//! он-чейн пула — агрегатор агрегирует много пулов; ядро эти поля для цены не использует.
//! Кто вызывает: venue.rs (subscribe_market).
//! От чего зависит: ctx.rs, state.rs, rest.rs, node.rs, balances.rs, quote.rs, num.rs, core/types, gateway/events.
//! Настройки: kyber.quote_poll_ms, kyber.quote_concurrency, kyber.probe_usdt, kyber.assume_zero_transfer_tax [kyberswap].

use std::collections::HashSet;
use std::time::Duration;

use alloy_primitives::{Address, U256};
use chrono::Utc;
use futures_util::future::join_all;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    Asset, CheckState, CoinCheck, Pool, PoolKind, PoolRef, PoolState, TokenTaxes, VenueId,
};
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::balances::decimals;
use super::rest::{self, Route};
use super::state::Parts;
use super::{node, num, quote};

/// Запускает поллинг квот. Проверяет обязательные настройки (иначе NotConfigured — ядро повторит подписку при их
/// задании), запоминает монеты, останавливает прежнюю задачу и ставит новую. Ошибки: NotConfigured.
pub async fn subscribe(
    p: &Parts,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
) -> GatewayResult<()> {
    // Обязательные настройки читаются до запуска: не заданы — подписки нет, причина видна (ядро повторит).
    let _ = p.ctx.int("kyber.quote_poll_ms")?;
    let _ = p.ctx.int("kyber.quote_concurrency")?;
    let _ = p.ctx.dec("kyber.probe_usdt")?;
    p.set_assets(assets.clone());
    let task = tokio::spawn(run(p.clone(), assets, out));
    p.swap_poll(task);
    Ok(())
}

/// Цикл поллинга: пока жив канал ядра. Сбой одного цикла — запись и пауза, не остановка.
async fn run(p: Parts, assets: Vec<Asset>, out: mpsc::Sender<MarketEvent>) {
    let mut last_err: Option<String> = None;
    loop {
        if out.is_closed() {
            return;
        }
        let poll = p
            .ctx
            .int("kyber.quote_poll_ms")
            .map(Duration::from_millis)
            .unwrap_or_else(|_| Duration::from_secs(2));
        match cycle(&p, &assets, &out).await {
            Ok(()) => {
                if last_err.take().is_some() {
                    p.ctx
                        .log(Level::Info, "kyber.poll", "Поллинг квот снова работает")
                        .await;
                }
            }
            Err(e) if last_err.as_deref() != Some(e.message.as_str()) => {
                p.ctx
                    .log(
                        Level::Warn,
                        "kyber.poll",
                        format!("Поллинг квот: {}", e.message),
                    )
                    .await;
                last_err = Some(e.message);
            }
            Err(_) => {}
        }
        tokio::time::sleep(poll).await;
    }
}

/// Один проход по монетам: маршрут USDT→монета на пробный объём → событие Pool; пропавшие — Invalidated; новые при
/// включённом kyber.assume_zero_transfer_tax — CoinCheck. Ошибки: NotConfigured, узел (USDT/decimals/блок).
async fn cycle(p: &Parts, assets: &[Asset], out: &mpsc::Sender<MarketEvent>) -> GatewayResult<()> {
    let ctx = &p.ctx;
    let usdt = quote::usdt(p)?;
    let usdt_dec = decimals(p, usdt).await?;
    let probe = ctx.dec("kyber.probe_usdt")?;
    let probe_units = num::to_units(probe, usdt_dec).ok_or_else(|| {
        ctx.not_set(
            "kyber.probe_usdt",
            "kyber.probe_usdt не переводится в единицы USDT",
        )
    })?;
    let block = node::block_number(ctx).await.unwrap_or(0);
    let concurrency = ctx.int("kyber.quote_concurrency")?.max(1) as usize;

    let coins: Vec<Asset> = assets
        .iter()
        .filter(|a| a.bsc_contract.is_some_and(|c| c != usdt))
        .cloned()
        .collect();

    let mut routed: HashSet<Address> = HashSet::new();
    for chunk in coins.chunks(concurrency) {
        let results = join_all(chunk.iter().map(|a| probe_one(p, a, usdt, probe_units))).await;
        for (asset, res) in chunk.iter().zip(results) {
            let token = asset.bsc_contract.expect("отфильтровано выше");
            if let Some(route) = res {
                routed.insert(token);
                let pool = marker(ctx.venue(), usdt, token, probe_units, &route, block);
                if out.send(MarketEvent::Pool(pool)).await.is_err() {
                    return Ok(());
                }
            }
        }
    }

    let (gone, entered) = p.update_routed(routed);
    for token in gone {
        if let Some(a) = coins.iter().find(|a| a.bsc_contract == Some(token)) {
            let _ = out
                .send(MarketEvent::Invalidated {
                    venue: ctx.venue(),
                    ticker: a.ticker.clone(),
                })
                .await;
        }
    }
    // Новым монетам — отметка о налогах, если человек разрешил считать их нулевыми (иначе «налоги не проверены»).
    if ctx
        .venue_flag("kyber.assume_zero_transfer_tax")
        .unwrap_or(false)
    {
        for token in entered {
            if let Some(a) = coins.iter().find(|a| a.bsc_contract == Some(token)) {
                let _ = out
                    .send(MarketEvent::CoinCheck(zero_taxes(ctx.venue(), a)))
                    .await;
            }
        }
    }
    Ok(())
}

/// Один маршрут USDT→монета на пробный объём. None — маршрута нет или сбой (монета выпадет из routed). Ошибок нет.
async fn probe_one(p: &Parts, asset: &Asset, usdt: Address, probe_units: U256) -> Option<Route> {
    let token = asset.bsc_contract?;
    rest::routes(&p.ctx, usdt, token, probe_units).await.ok()
}

/// Маркер пула-агрегатора: V2-форма без крючка (ядро допускает направление), резервы = пробные вход/выход маршрута
/// (реальные числа агрегатора, не чтение он-чейн пула; ядро их для цены не использует), комиссия 0 (внутри котировки).
fn marker(
    venue: VenueId,
    usdt: Address,
    coin: Address,
    probe_units: U256,
    route: &Route,
    block: u64,
) -> Pool {
    // Порядок валют: currency0 < currency1 по адресу (как в V2/V3/V4).
    let (token0, token1, reserve0, reserve1) = if coin < usdt {
        (coin, usdt, route.amount_out, probe_units)
    } else {
        (usdt, coin, probe_units, route.amount_out)
    };
    Pool {
        venue,
        family: "kyberswap_aggregator".to_string(),
        kind: PoolKind::V2,
        reference: PoolRef::Contract(coin),
        token0,
        token1,
        fee_pips: Some(0),
        tick_spacing: None,
        bin_step: None,
        hooks: None,
        state: PoolState::V2 { reserve0, reserve1 },
        block,
        received_at: Utc::now(),
    }
}

/// CoinCheck с нулевыми налогами (решение 91): налог свопа — внутри котировки агрегатора, налог перевода принят
/// нулевым человеком (kyber.assume_zero_transfer_tax). Ошибок нет.
fn zero_taxes(venue: VenueId, asset: &Asset) -> CoinCheck {
    CoinCheck {
        venue,
        asset: asset.clone(),
        taxes: CheckState::Known(TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        }),
        source: "kyberswap_aggregator: налоги в котировке маршрута; налог перевода принят нулевым (kyber.assume_zero_transfer_tax)".to_string(),
        checked_at: Some(Utc::now()),
        wait: None,
    }
}
