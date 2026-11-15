//! Файл: venues/bsc_dex/math/dmm.rs
//! Что делает: формулы пула KyberSwap Classic DMM (решение 287) — выход по входу и вход по выходу по ВИРТУАЛЬНЫМ
//! резервам и комиссии блока (feeInPrecision над 10^18), с проверкой «выход меньше РЕАЛЬНОГО резерва».
//! Кто вызывает: quote/pool_quote.rs (ветка AmpV2), quote/depth_band.rs; тесты на записанных ответах цепи.
//! От чего зависит: alloy-primitives.
//! Настройки: нет (резервы и комиссия — из состояния пула на блоке, pools/load_dmm.rs).
//!
//! Источник (решение 23): github.com/KyberNetwork/dmm-smart-contracts@e557b57, contracts/libraries/DMMLibrary.sol —
//! getAmountOut L75-L91, getAmountIn L93-L111 (SafeMath: переполнение и вычитание ниже нуля — откат → None);
//! PRECISION = 10^18 (MathExt.sol L9). Развёрнутый в BSC код: DMMRouter02 0x78df70615ffc8066cc0887917f2cd72092c86409
//! (Sourcify match; его getAmountsOut совпал с этой формулой на блоке 133680633 — tests/data/kyber_dmm_2026-11-15).
//! Пул проверяет K по тем же виртуальным резервам и той же комиссии (DMMPool.sol verifyBalanceAndUpdateEma L285-L309),
//! комиссия обмена в блоке равна feeInPrecision из getTradeInfo этого блока (L240-L265: rFactor по block.number; объём
//! самого обмена в комиссию этого обмена не входит — VolumeTrendRecorder.sol L50-L81).

use alloy_primitives::U256;

/// PRECISION = 10^18 — точность комиссии пула (MathExt.sol L9), свойство протокола, не настройка.
pub fn precision() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}

/// Резервы одной стороны обмена: реальные (вход, выход) и виртуальные (вход, выход).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Side {
    pub r_in: U256,
    pub r_out: U256,
    pub v_in: U256,
    pub v_out: U256,
}

/// getAmountOut (DMMLibrary.sol L75-L91). Принимает: вход, резервы стороны, комиссию над 10^18. Возвращает: выход.
/// None — как откат контракта: нулевой вход или реальный резерв, комиссия больше 100 %, переполнение, выход не меньше
/// реального резерва выхода. Ошибок иных нет.
pub fn amount_out(amount_in: U256, s: Side, fee: U256) -> Option<U256> {
    let p = precision();
    if amount_in.is_zero() || s.r_in.is_zero() || s.r_out.is_zero() || fee > p {
        return None;
    }
    let with_fee = amount_in.checked_mul(p - fee)? / p;
    let num = with_fee.checked_mul(s.v_out)?;
    let den = s.v_in.checked_add(with_fee)?;
    if den.is_zero() {
        return None;
    }
    let out = num / den;
    (s.r_out > out).then_some(out)
}

/// getAmountIn (DMMLibrary.sol L93-L111). Принимает: выход, резервы стороны, комиссию над 10^18. Возвращает: вход.
/// None — как откат контракта: нулевой выход, нулевой реальный резерв входа, выход не меньше реального резерва выхода
/// или виртуального, комиссия не меньше 100 %, переполнение. Ошибок иных нет.
pub fn amount_in(amount_out: U256, s: Side, fee: U256) -> Option<U256> {
    let p = precision();
    if amount_out.is_zero() || s.r_in.is_zero() || s.r_out <= amount_out || fee >= p {
        return None;
    }
    let num = s.v_in.checked_mul(amount_out)?;
    let den = s.v_out.checked_sub(amount_out)?;
    if den.is_zero() {
        return None;
    }
    let first = num / den + U256::from(1u8);
    let num = first.checked_mul(p)?;
    let den = p - fee;
    Some(num.checked_add(den - U256::from(1u8))? / den)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(x: u128) -> U256 {
        U256::from(x)
    }

    /// Выход не меньше реального резерва — откат (require reserveOut > amountOut), даже если виртуальный больше.
    #[test]
    fn real_reserve_caps_output() {
        let s = Side {
            r_in: u(1_000),
            r_out: u(10),
            v_in: u(1_000_000),
            v_out: u(1_000_000),
        };
        assert_eq!(amount_out(u(100), s, U256::ZERO), None);
        assert!(amount_out(u(5), s, U256::ZERO).is_some());
    }

    /// Обратная формула даёт вход, на котором прямая отдаёт не меньше заказанного (округления контракта).
    #[test]
    fn inverse_covers_output() {
        let s = Side {
            r_in: u(5_000_000_000_000_000_000),
            r_out: u(7_000_000_000_000_000_000),
            v_in: u(50_000_000_000_000_000_000),
            v_out: u(70_000_000_000_000_000_000),
        };
        let fee = u(1_505_309_943_994_589);
        let want = u(123_456_789_000_000_000);
        let need = amount_in(want, s, fee).unwrap();
        assert!(amount_out(need, s, fee).unwrap() >= want);
    }
}
