//! Файл: venues/bsc_dex/pools/events_stable_u.rs
//! Что делает: события пулов PancakeSwap StableSwap, Lista StableSwap, Saddle (Nerve), общего контракта WooFi
//! (решение 122 F, 3) и крючка PancakeSwap Infinity Stable (решение «122 (Infinity Stable)»; пул живёт в крючке,
//! события выпускает он — подписка по адресу крючка, Infinity Stable тоже перечитывается каждым блоком) — topic0
//! для подписки и применение: любое известное событие пула → перечитать пул целиком (балансы, A, комиссия, пауза; у WooFi — оракул, пауза и реализация прокси). Разбирать суммы событий не нужно:
//! перечитывание на блоке события точно. Lista и WooFi ещё перечитываются на каждом новом блоке (цена оракула меняется
//! без событий пула — pools/session.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_stable_u.rs, abi_saddle.rs, abi_woo.rs, abi_inf_stable.rs, abi_curve.rs (MAX_COINS), events.rs,
//! registry.rs.
//! Настройки: нет.
//!
//! Сигнатуры — abi_stable_u.rs, abi_saddle.rs, abi_woo.rs, abi_inf_stable.rs (ссылки на официальные исходники там).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use super::abi_curve::MAX_COINS;
use super::abi_inf_stable as inf_stable;
use super::abi_saddle::ISaddleSwap as S;
use super::abi_stable_u::{IListaStable as L, IPancakeStable as P, liquidity_topics as liq};
use super::abi_woo::IWooPP as W;
use super::events::Applied;
use super::families::Proto;
use super::registry::Entry;

/// topic0 всех событий пула вида. Принимает: протокол. Возвращает: список (пусто — вид не из этого файла). Ошибок нет.
pub fn topics(p: Proto) -> Vec<B256> {
    let mut out = match p {
        Proto::PancakeStable => vec![
            P::TokenExchange::SIGNATURE_HASH,
            P::RemoveLiquidityOne::SIGNATURE_HASH,
            P::NewFee::SIGNATURE_HASH,
            P::RampA::SIGNATURE_HASH,
            P::StopRampA::SIGNATURE_HASH,
            P::Kill::SIGNATURE_HASH,
            P::Unkill::SIGNATURE_HASH,
        ],
        Proto::ListaStable => vec![
            L::TokenExchange::SIGNATURE_HASH,
            L::RemoveLiquidityOne::SIGNATURE_HASH,
            L::NewFee::SIGNATURE_HASH,
            L::RampA::SIGNATURE_HASH,
            L::StopRampA::SIGNATURE_HASH,
            L::ChangePriceDiffThreshold::SIGNATURE_HASH,
            L::SetSkipPriceDiff::SIGNATURE_HASH,
            L::ChangeOracle::SIGNATURE_HASH,
            L::Paused::SIGNATURE_HASH,
            L::Unpaused::SIGNATURE_HASH,
        ],
        Proto::SaddleStable => vec![
            S::TokenSwap::SIGNATURE_HASH,
            S::AddLiquidity::SIGNATURE_HASH,
            S::RemoveLiquidity::SIGNATURE_HASH,
            S::RemoveLiquidityOne::SIGNATURE_HASH,
            S::RemoveLiquidityImbalance::SIGNATURE_HASH,
            S::NewAdminFee::SIGNATURE_HASH,
            S::NewSwapFee::SIGNATURE_HASH,
            S::RampA::SIGNATURE_HASH,
            S::StopRampA::SIGNATURE_HASH,
            S::Paused::SIGNATURE_HASH,
            S::Unpaused::SIGNATURE_HASH,
        ],
        Proto::WooPP => vec![
            W::WooSwap::SIGNATURE_HASH,
            W::Upgraded::SIGNATURE_HASH,
            W::Paused::SIGNATURE_HASH,
            W::Unpaused::SIGNATURE_HASH,
            W::WooracleUpdated::SIGNATURE_HASH,
        ],
        // Infinity Stable (решение «122 (Infinity Stable)»): события крючка — обмен, A, комиссия (abi_inf_stable.rs).
        Proto::InfinityStable => return inf_stable::topics(),
        _ => Vec::new(),
    };
    out.extend(liquidity_topics(p));
    out
}

/// topic0 событий изменения ликвидности (их слушают мелкие пулы, pools/depth.rs). Ошибок нет.
pub fn liquidity_topics(p: Proto) -> Vec<B256> {
    match p {
        // PancakeSwap — двух- и трёхмонетные пулы (Add/Remove/Imbalance с массивами uint256[n]).
        Proto::PancakeStable => (2..=MAX_COINS).flat_map(|n| liq(n, false)).collect(),
        // Lista — только два монеты (N_COINS = 2, StableSwapPool.sol L24).
        Proto::ListaStable => liq(2, true),
        Proto::SaddleStable => vec![S::AddLiquidity::SIGNATURE_HASH],
        Proto::WooPP => Vec::new(),
        Proto::InfinityStable => inf_stable::liquidity_topics(),
        _ => Vec::new(),
    }
}

/// Применяет событие: известное событие пула → Refresh. Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics(e.family.proto).contains(t) => Applied::Refresh(format!(
            "{}: событие пула — перечитать состояние",
            e.family.title
        )),
        _ => Applied::Ignored,
    }
}
