//! Файл: venues/bsc_dex/math/algebra_fee.rs
//! Что делает: решение 444 — комиссия обмена пула Algebra по направлению. У Algebra V1.9 с направленными комиссиями
//! (Camelot V3, вид algebra_dir_fee) контракт берёт на весь своп одну из двух комиссий globalState: token0 → token1 —
//! feeZto, token1 → token0 — feeOtz. У видов с одной комиссией (THENA Fusion, Algebra Integral) — одна на обе стороны.
//! Кто вызывает: quote/pool_quote.rs (котировка — комиссия шага math/swap_algebra), quote/steps.rs (строка затрат).
//! От чего зависит: ничего (чистая функция).
//! Настройки: нет.
//!
//! Источник: cryptoalgebra/AlgebraV1.9, ветка direction-based-fee, коммит a45417ed5048c0a9ad3c673e41a00bb3c3880f6f
//! (официальная «Algebra V1.9 — directional fees», docs.algebra.finance api-reference-v1.9-directional-fees;
//! Camelot: docs.camelot.exchange/protocol/amm-v3 — «based on Algebra's V1.9», направленные комиссии):
//! src/core/contracts/AlgebraPool.sol:739 — `cache.fee = zeroToOne ? globalState.feeZto : globalState.feeOtz;`;
//! дальше cache.fee идёт во все шаги цены (AlgebraPool.sol:816) — как fee у V1.9 без направлений.

/// Комиссия обмена (пипсы, 1e-6) по направлению. Принимает: комиссию token0 → token1 (`fee`; у видов с одной
/// комиссией — единственную), комиссию token1 → token0 (`fee_otz`; None — одна комиссия на обе стороны), направление
/// (true — token0 → token1). Возвращает: комиссию, которую контракт возьмёт на весь своп. Ошибок нет.
pub fn swap_fee(fee: u32, fee_otz: Option<u32>, zero_for_one: bool) -> u32 {
    match (zero_for_one, fee_otz) {
        (false, Some(otz)) => otz,
        _ => fee,
    }
}

#[cfg(test)]
mod tests {
    use super::swap_fee;

    /// AlgebraPool.sol:739: zeroToOne — feeZto, иначе feeOtz; без второй комиссии — одна на обе стороны.
    #[test]
    fn picks_by_direction() {
        assert_eq!(swap_fee(100, Some(2500), true), 100);
        assert_eq!(swap_fee(100, Some(2500), false), 2500);
        assert_eq!(swap_fee(500, None, true), 500);
        assert_eq!(swap_fee(500, None, false), 500);
    }
}
