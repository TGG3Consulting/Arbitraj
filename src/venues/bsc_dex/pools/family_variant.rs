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
    /// Решение 470 (T451-01): Aero «Slipstream V3» (dex.kind = slipstream_aero; Dromos-Labs/metadex-slipstream-public
    /// 1.1.0, развёртывание Arbitrum cl-arbitrum.json): тот же CLPool, что у вида slipstream (slot0, ticks, getPool(int24),
    /// PoolCreated, Swap, цикл обмена), но комиссия обмена — от крючка (DynamicSwapFeeHook): меняется по блокам без
    /// событий пула — комиссия читается каждым блоком, крючок пула и правило MEV-налога проверяются (pools/aero_cl.rs).
    SlipstreamAero,
    /// Curve StableSwap 2021 (dex.kind = curve_stableswap_plain, решение 472): протокол Proto::CurveStable (геттеры
    /// coins/balances/fee, get_dy и exchange с int128 — те же), но A — `A_precise()` (A·A_PRECISION, A_PRECISION = 100),
    /// несходимость D/y — откат (`raise`), у пула из двух монет D_P — одной строкой `D·D/x0·D/x1/4`. Исходники: 2pool
    /// Arbitrum 0x7f90122BF0700F9E7e1F688fe926940E8839F353 (Vyper 0.2.12, Sourcify 42161 match) и Plain2Basic /
    /// Plain3Basic / Plain4Basic фабрики Curve Sidechain/L2 (curvefi/curve-factory f389123, Vyper 0.2.15; Sourcify
    /// 42161 match реализаций 0x54e8…7A62, 0xe381…7A62, 0x8d53…fa86). Чтение — load_curve_plain.rs, проверка —
    /// verify_curve_plain.rs, события — events_curve.rs (topics_of), состояние — каждым блоком (per_block.rs).
    CurvePlain,
    /// Метапул фабрики Curve Sidechain/L2 (dex.kind = curve_stableswap_meta, решение 472, доделка): тот же протокол и
    /// шаблон, что CurvePlain, но курс монеты 1 (LP-токен базового пула) — get_virtual_price() базового пула на блоке,
    /// D_P — циклом. Исходник — MetaUSD.vy / MetaUSDBalances.vy (curvefi/curve-factory f389123); чтение —
    /// load_curve_meta.rs, проверка — verify_curve_plain.rs (реализация из dex.curve_plain_implementations и
    /// get_base_pool фабрики = coins(1)), события — events_curve.rs, состояние — каждым блоком (per_block.rs).
    CurveMeta,
    /// Пул Curve StableSwap с курсом оракула и нативным ETH (dex.kind = curve_stableswap_oracle, решение 480, T472-10):
    /// ETH/wstETH Arbitrum 0x6eB2dc694eB516B16Dc9FBc678C60052BbdD7d80 (Vyper 0.3.1, Sourcify 42161 creationMatch match,
    /// runtimeMatch null; docs/sources/curve_arbitrum/). Протокол Proto::CurveStable (coins/balances/fee, A_precise,
    /// exchange(int128, int128, dx, min_dy) — те же), пул — из списка dex.stable_pools; отличие от CurvePlain: курс
    /// монеты 1 — `Oracle(ORACLE).latestAnswer()` на блоке (`_stored_rates` L193-L199; адрес — `oracle()` пула L851,
    /// подтверждается проверкой), курс монеты 0 (метка ETH 0xEeee…, `__init__` L144) — 10^18, D_P — циклом (get_D
    /// L213-L239). Чтение — load_curve_oracle.rs, проверка — verify_curve_plain.rs, состояние — каждым блоком (курс
    /// оракула меняется без событий пула, per_block.rs), шаг — CURVE_ETH (v5).
    CurveOracle,
    /// Двухмонетный crypto-пул Curve первого поколения без фабрики (dex.kind = curve_crypto2_v1, решение 472, T472-10):
    /// протокол Proto::CurveTricryptoV1 (пул из списка dex.stable_pools, монеты — coins(k), is_killed), но монет две и
    /// математика — CurveCryptoSwap2 (CryptoMath::TwoV1). Исходник — EURS/2CRV Arbitrum
    /// 0xA827a652Ead76c6B0b3D19dba05452E06e25c27e: curvefi/curve-crypto-contract 41e6289 «deployment-logs/2021-10-30.
    /// EURS/CryptoSwap.vy» (Vyper 0.3.0; адрес — deployment.log того же каталога): geometric_mean, newton_D, newton_y,
    /// halfpow, get_xcp, _A_gamma — тот же текст, что CurveCryptoSwap2ETH.vy (curve-factory-crypto 7ed1011; сверка
    /// функций скриптом), exchange L686-L765 — те же действия, что `_exchange` 2ETH (+ assert not is_killed L688).
    /// Чтение — load_crypto_v1.rs (n = 2 и is_killed()), модель tweak_price v1 фабрики к нему не применяется.
    CryptoPair,
}

/// Вариант по значению dex.kind. Принимает: значение dex.kind. Возвращает: вариант (неизвестное — Base). Ошибок нет.
pub fn of(kind: &str) -> Variant {
    match kind {
        "kyber_elastic_v1" => Variant::ElasticV1,
        "slipstream_aero" => Variant::SlipstreamAero,
        "curve_stableswap_plain" => Variant::CurvePlain,
        "curve_stableswap_meta" => Variant::CurveMeta,
        "curve_stableswap_oracle" => Variant::CurveOracle,
        "curve_crypto2_v1" => Variant::CryptoPair,
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
        assert_eq!(of("slipstream"), Variant::Base);
        assert_eq!(of("slipstream_aero"), Variant::SlipstreamAero);
        assert_eq!(of("curve_stableswap"), Variant::Base);
        assert_eq!(of("curve_stableswap_plain"), Variant::CurvePlain);
        assert_eq!(of("curve_stableswap_meta"), Variant::CurveMeta);
        assert_eq!(of("curve_stableswap_oracle"), Variant::CurveOracle);
        assert_eq!(of("curve_crypto2_v1"), Variant::CryptoPair);
        assert_eq!(of("curve_tricrypto_v1"), Variant::Base);
    }
}
