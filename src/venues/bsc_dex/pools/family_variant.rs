//! Файл: venues/bsc_dex/pools/family_variant.rs
//! Что делает: решение 451 — вариант исходника внутри одного протокола вида пула (Proto): тот же ABI чтения, событий и
//! проверки, но другое правило математики у развёрнутого кода. Вариант задаётся значением dex.kind (отдельное значение
//! на вариант — Тигран выбирает его в интерфейсе, как вид), хранится в виде DEX (Family::variant) и читается
//! математикой на горячем пути без обращения к хранилищу настроек.
//! Кто вызывает: pools/families.rs (make — при создании вида), quote/pool_quote.rs, quote/depth_band.rs.
//! От чего зависит: нет.
//! Настройки: dex.kind [DEX] (значение выбирает вариант).
//!
//! Варианты (официальные исходники — в комментариях вариантов).

/// Вариант исходника вида пула. Base — исходник, по которому вид сделан (у всех прежних dex.kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// Исходник, по которому сделан вид (поведение без изменений).
    Base,
    /// KyberSwap Elastic v1 (dex.kind = kyber_elastic_v1): код фабрики 0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a в любой
    /// сети (KyberNetwork/ks-elastic-sc 4ab08c0a README.md L6-L17 «v1» — в т.ч. Arbitrum и BSC; Sourcify 42161 full
    /// match, solc 0.8.9). Elastic v2 (вид kyber_elastic) — например фабрика 0xC7a590291e07B9fe9E64b86c58fD8fC764308C4A на
    /// BSC (Sourcify 56: SwapMath.sol `usedAmount > specifiedAmount`). Вид выбирается по коду фабрики, а не по сети.
    /// Отличие от вида (HEAD 4ab08c0, Elastic v2) в расчёте сумм обмена одно: SwapMath.computeSwapStep v1 —
    /// `(isExactInput && usedAmount >= specifiedAmount)`, v2 — `>` (ks-elastic-sc 4ab08c0 contracts/libraries/
    /// SwapMath.sol L60; первый коммит v2 0e7713a — ещё `>=`). Остальное (calcReachAmount, estimate/
    /// calcIncrementalLiquidity, calcFinalPrice, calcReturnedAmount, QuadMath, FullMath, MAX_TICK_DISTANCE = 480,
    /// цикл Pool.swap, _getInitialSwapData, _updateLiquidityAndCrossTick, раскладка PoolStorage) — то же. Отличия вне
    /// расчёта сумм: тик после неполного шага (у v1 нет startSqrtP), MIN_LIQUIDITY, длина массива TicksFeesReader.
    ElasticV1,
}

/// Вариант по значению dex.kind. Принимает: значение dex.kind. Возвращает: вариант (неизвестное — Base). Ошибок нет.
pub fn of(kind: &str) -> Variant {
    match kind {
        "kyber_elastic_v1" => Variant::ElasticV1,
        _ => Variant::Base,
    }
}

#[cfg(test)]
mod tests {
    use super::{Variant, of};

    /// Прежние значения dex.kind остаются Base (поведение BSC не меняется), v1 — свой вариант.
    #[test]
    fn variant_by_kind() {
        assert_eq!(of("kyber_elastic"), Variant::Base);
        assert_eq!(of("saddle_stableswap"), Variant::Base);
        assert_eq!(of("kyber_elastic_v1"), Variant::ElasticV1);
    }
}
