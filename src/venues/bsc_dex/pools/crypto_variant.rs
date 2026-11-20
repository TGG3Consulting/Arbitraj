//! Файл: venues/bsc_dex/pools/crypto_variant.rs
//! Что делает: выбор версии формул crypto-пула Curve NG по MATH() пула (решение 453): у фабрики Twocrypto-NG Arbitrum
//! две реализации математики (v2.0.0 и v2.1.0, deployments.yaml `math_v2_0_0` / `math`), формулы get_y / newton_D у них
//! отличаются пределами assert'ов (math/crypto_lim2.rs). Пул зовёт свою математику (MATH() — immutable пула), поэтому
//! версия определяется адресом его математики, а не version() пула.
//! Кто вызывает: pools/load_crypto.rs (state), pools/verify_crypto.rs (список допустимых адресов математики).
//! От чего зависит: core/types (CryptoMath), alloy-primitives.
//! Настройки: dex.curve_crypto_math, dex.curve_crypto_math_v2_0_0 [DEX] (имена — здесь).

use alloy_primitives::Address;

use crate::core::types::CryptoMath;

/// Настройка адресов математики Twocrypto-NG v2.0.0 (решение 453).
pub const MATH_V200: &str = "dex.curve_crypto_math_v2_0_0";

/// Что делает: версия формул по MATH() пула (решение 453). Принимает: число монет, MATH() пула, адреса математики
/// v2.0.0 (dex.curve_crypto_math_v2_0_0), адреса dex.curve_crypto_math. Возвращает: версию (2 монеты: v2.0.0 или
/// v2.1.0; 3 — Tricrypto-NG) или текст «формула не сверена».
pub fn variant(
    n: usize,
    math: Address,
    v200: &[Address],
    ng: &[Address],
) -> Result<CryptoMath, String> {
    if n == 2 && v200.contains(&math) {
        return Ok(CryptoMath::TwoNg200);
    }
    if ng.contains(&math) {
        return Ok(if n == 2 { CryptoMath::TwoNg210 } else { CryptoMath::TriNg });
    }
    Err(format!(
        "MATH() пула {math:#x} нет в dex.curve_crypto_math / {MATH_V200} — формула не сверена"
    ))
}
