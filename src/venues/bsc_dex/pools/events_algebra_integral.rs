//! Файл: venues/bsc_dex/pools/events_algebra_integral.rs
//! Что делает: решение 188 — живое состояние пула Algebra Integral по событиям. Swap/Mint/Burn у Integral те же, что у
//! V1.9 (IAlgebraPoolEvents.sol integral-core 1.2.10, L22-L80) — их применяет events_algebra.rs (Swap — цена,
//! ликвидность, тик; Mint/Burn — перечитать пул). Своё у Integral — комиссия:
//! - SwapFee(sender, overrideFee, pluginFee) — с какой комиссией прошёл обмен (SwapCalculation.sol: overrideFee ≠ 0 →
//!   fee = overrideFee + pluginFee). overrideFee ≠ 0 — берём эту сумму как текущую комиссию (факт последнего обмена);
//!   overrideFee = 0, pluginFee ≠ 0 — сумма зависит от lastFee, которой в событии нет: пул перечитывается (Refresh);
//!   оба нуля — комиссия = lastFee, не меняется.
//! - Версия 1.2.0/1.2.1 (THENA V3 — живые логи 06.11.2026, tests/data/algebra_integral_2026-11-06): отдельного SwapFee
//!   нет — overrideFee и pluginFee приходят в самом Swap (9 полей, IAlgebraIntegral12Pool): цена/ликвидность/тик
//!   применяются как у Swap V1.9, комиссия — по тем же правилам, что SwapFee; Burn с полем pluginFee — перечитать пул.
//! - Fee(uint16 fee) — новая lastFee: пул перечитывается (у плагина с DYNAMIC_FEE комиссию определяет fee() плагина,
//!   а не lastFee — load_algebra_integral.rs).
//!
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_algebra_integral.rs, pools/events_algebra.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::PoolState;

use super::abi_algebra_integral::{IAlgebraIntegral12Pool as P12, IAlgebraIntegralPool as P};
use super::events::Applied;
use super::events_algebra;
use super::registry::Entry;

/// topic0 всех событий пула Integral: Swap/Mint/Burn (как V1.9) + SwapFee + Fee (1.2.2+) и Swap/Burn версии
/// 1.2.0/1.2.1. Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut t = events_algebra::topics();
    t.extend([
        P::SwapFee::SIGNATURE_HASH,
        P::Fee::SIGNATURE_HASH,
        P12::Swap::SIGNATURE_HASH,
        P12::Burn::SIGNATURE_HASH,
    ]);
    t
}

/// topic0 добавления ликвидности — Mint, как у V1.9. Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    events_algebra::liquidity_topics()
}

/// Применяет событие Integral (см. шапку). Ошибок нет: не разобрано — Refresh.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    if t0 == P::Fee::SIGNATURE_HASH {
        return Applied::Refresh("Algebra Integral Fee: перечитать комиссию пула".into());
    }
    if t0 == P12::Burn::SIGNATURE_HASH {
        return Applied::Refresh(
            "Algebra Integral Burn: перечитать ликвидность и карту тиков".into(),
        );
    }
    if t0 == P12::Swap::SIGNATURE_HASH {
        return match P12::Swap::decode_log_data(&log.inner.data) {
            Ok(s) => swap12(
                e,
                U256::from(s.price),
                s.liquidity,
                s.tick.as_i32(),
                fee_after(s.overrideFee.to::<u32>(), s.pluginFee.to::<u32>()),
            ),
            Err(err) => Applied::Refresh(format!("Swap (Integral 1.2.0/1.2.1) не по ABI: {err}")),
        };
    }
    if t0 != P::SwapFee::SIGNATURE_HASH {
        return events_algebra::apply(e, log);
    }
    let s = match P::SwapFee::decode_log_data(&log.inner.data) {
        Ok(s) => s,
        Err(err) => return Applied::Refresh(format!("SwapFee не по ABI: {err}")),
    };
    let (over, plugin) = (s.overrideFee.to::<u32>(), s.pluginFee.to::<u32>());
    match fee_after(over, plugin) {
        Swapfee::Keep => Applied::Ignored,
        Swapfee::Reload => Applied::Refresh(
            "Algebra Integral SwapFee: добавка плагина к lastFee — перечитать комиссию".into(),
        ),
        Swapfee::Set(v) => match &mut e.pool.state {
            PoolState::Algebra { fee, .. } => {
                *fee = v;
                Applied::Updated
            }
            _ => Applied::Refresh("состояние пула не Algebra".into()),
        },
    }
}

/// Swap версии 1.2.0/1.2.1: цена, ликвидность, тик после обмена и комиссия по правилам SwapFee. Ошибок нет.
fn swap12(e: &mut Entry, sqrt: U256, liq: u128, t: i32, f: Swapfee) -> Applied {
    let PoolState::Algebra {
        sqrt_price_x96,
        tick,
        liquidity,
        fee,
    } = &mut e.pool.state
    else {
        return Applied::Refresh("состояние пула не Algebra".into());
    };
    *sqrt_price_x96 = sqrt;
    *tick = t;
    *liquidity = liq;
    match f {
        Swapfee::Keep => Applied::Updated,
        Swapfee::Set(v) => {
            *fee = v;
            Applied::Updated
        }
        Swapfee::Reload => Applied::Refresh(
            "Algebra Integral Swap: добавка плагина к lastFee — перечитать комиссию".into(),
        ),
    }
}

/// Что делать с комиссией по SwapFee.
#[derive(Debug, PartialEq, Eq)]
pub enum Swapfee {
    /// Комиссия = lastFee, не меняется.
    Keep,
    /// Нужна lastFee, которой в событии нет — перечитать пул.
    Reload,
    /// Новая комиссия (пипсы, 1e-6).
    Set(u32),
}

/// Решение по полям SwapFee (SwapCalculation.sol, integral-core 1.2.10). Ошибок нет.
pub fn fee_after(override_fee: u32, plugin_fee: u32) -> Swapfee {
    match (override_fee, plugin_fee) {
        (0, 0) => Swapfee::Keep,
        (0, _) => Swapfee::Reload,
        (o, p) => Swapfee::Set(o + p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_fee_rules() {
        assert_eq!(fee_after(0, 0), Swapfee::Keep);
        assert_eq!(fee_after(0, 50), Swapfee::Reload);
        assert_eq!(fee_after(2500, 0), Swapfee::Set(2500));
        assert_eq!(fee_after(2500, 100), Swapfee::Set(2600));
    }
}
