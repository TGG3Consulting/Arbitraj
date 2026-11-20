//! Файл: venues/bsc_dex/pools/events_curve.rs
//! Что делает: живое состояние пула StableSwap (Ellipsis StableSwap; решение 122 F) по событиям. Любое событие,
//! меняющее балансы, A или комиссию (TokenExchange, AddLiquidity, RemoveLiquidity, RemoveLiquidityOne,
//! RemoveLiquidityImbalance, NewFee, RampA, StopRampA), ведёт к Refresh — пул перечитывается целиком (load_curve.rs).
//! Причина: TokenExchange несёт только суммы, а баланс выходной монеты меняется ещё и на долю администратора
//! (`balances[j] = old − dy − dy_admin_fee`, StableSwap.vy L466-L468) — точечно по событию балансы не восстановить.
//! Ограничение: во время плавной смены A (RampA → future_A_time) `A()` меняется с каждым блоком без событий —
//! расхождение ловит сверка с квотером пула (get_dy, решение 73) и перечитывание перед сделкой (решение 83).
//! Решение 472: вариант CurvePlain (curve_stableswap_plain — 2pool Arbitrum, Plain*Basic фабрики) слушает ещё
//! RemoveLiquidityOne с token_supply (4 поля) и CommitNewFee (abi_curve_plain.rs) — topics_of; пул этого варианта ещё и
//! перечитывается каждым блоком (рампа A по времени блока, donate_admin_fees 2pool без события — per_block.rs).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_curve.rs, pools/abi_curve_plain.rs, pools/family_variant.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use super::abi_curve::{IStableSwapV1, MAX_COINS, topics_for};
use super::abi_curve_plain::{ICurveMeta, ICurvePlain, ICurvePlainEma};
use super::events::Applied;
use super::family_variant::Variant;
use super::registry::Entry;

/// topic0 всех событий пула (все ведут к перечитыванию); события с массивами — для любого числа монет 2..=MAX_COINS.
/// Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut out = vec![
        IStableSwapV1::TokenExchange::SIGNATURE_HASH,
        IStableSwapV1::RemoveLiquidityOne::SIGNATURE_HASH,
        IStableSwapV1::NewFee::SIGNATURE_HASH,
        IStableSwapV1::RampA::SIGNATURE_HASH,
        IStableSwapV1::StopRampA::SIGNATURE_HASH,
    ];
    for n in 2..=MAX_COINS {
        let (add, rest) = topics_for(n);
        out.push(add);
        out.extend(rest);
    }
    out
}

/// topic0 событий пула по варианту исходника: Base — topics(), CurvePlain (решение 472) — ещё RemoveLiquidityOne из
/// 4 полей и CommitNewFee (Plain2Basic.vy f389123 L48-L52; 2pool Arbitrum — ABI curve-js ab3f1c2), события комиссии
/// реализаций с оракулом; CurveMeta — то же и TokenExchangeUnderlying. Решение 481: CurveOracle (ETH/wstETH) — как
/// CurvePlain (его события — TokenExchange, ликвидность (RemoveLiquidityOne из 3 полей — topics()), CommitNewFee из 3
/// полей, NewFee, RampA, StopRampA; исходник L21-L77). Ошибок нет.
pub fn topics_of(v: Variant) -> Vec<B256> {
    let mut out = topics();
    if matches!(v, Variant::CurvePlain | Variant::CurveMeta | Variant::CurveOracle) {
        out.push(ICurvePlain::RemoveLiquidityOne::SIGNATURE_HASH);
        out.push(ICurvePlain::CommitNewFee::SIGNATURE_HASH);
        // Решение 472 (доделка): реализации с оракулом — смена комиссии CommitNewFee(uint256) / ApplyNewFee(uint256)
        // (Plain2BasicEMA.vy 5272fd2 L74-L78).
        out.push(ICurvePlainEma::CommitNewFee::SIGNATURE_HASH);
        out.push(ICurvePlainEma::ApplyNewFee::SIGNATURE_HASH);
    }
    if v == Variant::CurveMeta {
        // Метапул: обмен через базовый пул (MetaUSD.vy f389123 L48-L53).
        out.push(ICurveMeta::TokenExchangeUnderlying::SIGNATURE_HASH);
    }
    out
}

/// topic0 добавления ликвидности (AddLiquidity для любого числа монет) — их слушают мелкие пулы (pools/depth.rs).
/// Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    (2..=MAX_COINS).map(|n| topics_for(n).0).collect()
}

/// Применяет событие: любое известное событие пула (своего варианта — topics_of) → Refresh (см. шапку). Ошибок нет.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    match log.topic0() {
        Some(t) if topics_of(e.family.variant).contains(t) => {
            Applied::Refresh("StableSwap: событие пула — перечитать балансы, A и комиссию".into())
        }
        _ => Applied::Ignored,
    }
}
