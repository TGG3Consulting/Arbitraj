//! Файл: venues/bsc_dex/math/lb_cap.rs
//! Что делает: решение 446 — проверка Liquidity Book V2.2, которой нет у V2.1: после обмена в бине (BinHelper.getAmounts)
//! ликвидность бина L = price · x + (y << 128) (getLiquidity) не больше MAX_LIQUIDITY_PER_BIN, иначе пул откатывает
//! обмен (BinHelper__MaxLiquidityPerBinExceeded); переполнение при сложении резервов (PackedUint128Math.add / sub) и при
//! счёте L (BinHelper__LiquidityOverflow) — тоже откат. Остальной расчёт обмена у V2.2 тот же, что у V2.1
//! (math/lb_swap.rs): getAmounts, getTotalFee, updateReferences, updateVolatilityAccumulator, getPriceFromId — без
//! изменений формул (diff v2.1.0..v2.2.0: только аргумент времени блока и эта проверка).
//! Кто вызывает: math/lb_swap.rs (swap_out_v22 — откат, depth_v22 — конец полосы).
//! От чего зависит: alloy-primitives.
//! Настройки: нет (число — константа контракта, не настройка).
//!
//! Источник: lfj-gg/joe-v2@v2.2.0 (1297c38) — src/libraries/BinHelper.sol L226-L280 (getAmounts, проверка L273-L279),
//! L136-L153 (getLiquidity: price · x с проверкой переполнения, y << 128 с проверкой переполнения);
//! src/libraries/Constants.sol L22-L24 (MAX_LIQUIDITY_PER_BIN); src/libraries/math/PackedUint128Math.sol L147-L155 (add:
//! переполнение любой половины — откат), L184-L192 (sub). На ветке main (067c6cc) путь обмена тот же (diff
//! v2.2.0..main меняет только mint — комиссию состава и вынос проверок в функции).

use alloy_primitives::U256;

/// Constants.MAX_LIQUIDITY_PER_BIN (Constants.sol L22-L24) = 652517431167196730109656255402446531916199230143859853796
/// 00384103134737 — младшие слова первыми (ревью 446, Н1: без разбора строки; равенство числу исходника — тест ниже).
pub const MAX_LIQUIDITY_PER_BIN: U256 = U256::from_limbs([
    0x6d33_6ca2_a775_b611,
    0xca44_773d_d596_d31a,
    0xe0d0_f4e4_00fc_e79a,
    0x0000_0974_5258_e83d,
]);

/// BinHelper.getLiquidity(x, y, price) (L136-L153). Принимает: резервы X и Y, цену бина (128.128). Возвращает:
/// L; None — переполнение (контракт откатился бы BinHelper__LiquidityOverflow).
pub fn liquidity(x: u128, y: u128, price: U256) -> Option<U256> {
    let mut l = U256::ZERO;
    if x > 0 {
        l = price.checked_mul(U256::from(x))?;
    }
    if y > 0 {
        l = l.checked_add(U256::from(y) << 128usize)?;
    }
    Some(l)
}

/// Проверка getAmounts V2.2 (L273-L279) в одном бине. Принимает: резервы бина (X, Y) до обмена, цену бина,
/// направление (true — вход X), вход с комиссией и выход бина. Возвращает: Some(()) — обмен проходит; None — пул
/// откатил бы обмен (переполнение половины резерва, переполнение L или L больше MAX_LIQUIDITY_PER_BIN).
pub fn check(
    reserves: (u128, u128),
    price: U256,
    swap_for_y: bool,
    amount_in: u128,
    amount_out: u128,
) -> Option<()> {
    let (rx, ry) = reserves;
    // binReserves.add(amountsInWithFees).sub(amountsOutOfBin): сложение и вычитание каждой половины с проверкой.
    let (x, y) = if swap_for_y {
        (rx.checked_add(amount_in)?, ry.checked_sub(amount_out)?)
    } else {
        (rx.checked_sub(amount_out)?, ry.checked_add(amount_in)?)
    };
    (liquidity(x, y, price)? <= MAX_LIQUIDITY_PER_BIN).then_some(())
}

#[cfg(test)]
mod tests {
    use alloy_primitives::U256;

    use super::MAX_LIQUIDITY_PER_BIN;

    /// Константа = число из исходника (lfj-gg/joe-v2@v2.2.0 src/libraries/Constants.sol L23-L24, как записано там).
    #[test]
    fn equals_constants_sol() {
        let src = "65251743116719673010965625540244653191619923014385985379600384103134737";
        assert_eq!(MAX_LIQUIDITY_PER_BIN, U256::from_str_radix(src, 10).unwrap());
    }
}
