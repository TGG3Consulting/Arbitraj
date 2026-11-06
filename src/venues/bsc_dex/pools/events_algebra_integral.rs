//! Файл: venues/bsc_dex/pools/events_algebra_integral.rs
//! Что делает: решение 188 — живое состояние пула Algebra Integral по событиям. Swap/Mint/Burn у Integral те же, что у
//! V1.9 (IAlgebraPoolEvents.sol integral-core 1.2.10, L22-L80) — их применяет events_algebra.rs (Swap — цена,
//! ликвидность, тик; Mint/Burn — перечитать пул). Своё у Integral — комиссия:
//! - SwapFee(sender, overrideFee, pluginFee) — с какой комиссией прошёл обмен (SwapCalculation.sol: overrideFee ≠ 0 →
//!   fee = overrideFee + pluginFee). overrideFee ≠ 0 — берём эту сумму как текущую комиссию (факт последнего обмена);
//!   overrideFee = 0, pluginFee ≠ 0 — сумма зависит от lastFee, которой в событии нет: пул перечитывается (Refresh);
//!   оба нуля — комиссия = lastFee, не меняется.
//! - Fee(uint16 fee) — новая lastFee: пул перечитывается (у плагина с DYNAMIC_FEE комиссию определяет fee() плагина,
//!   а не lastFee — load_algebra_integral.rs).
//!
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_algebra_integral.rs, pools/events_algebra.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use crate::core::types::PoolState;

use super::abi_algebra_integral::IAlgebraIntegralPool as P;
use super::events::Applied;
use super::events_algebra;
use super::registry::Entry;

/// topic0 всех событий пула Integral: Swap/Mint/Burn (как V1.9) + SwapFee + Fee. Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut t = events_algebra::topics();
    t.extend([P::SwapFee::SIGNATURE_HASH, P::Fee::SIGNATURE_HASH]);
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
