//! Файл: venues/bsc_dex/pools/ekubo_native.rs
//! Что делает: нативный ETH в пулах Ekubo (решение 454, по образцу решения «122 X» — нативный BNB в стейблсвапах).
//! У ядра нативная монета — token0 = адрес 0 (NATIVE_TOKEN_ADDRESS); бот работает только с токенами, поэтому в паре
//! пула и в графе вместо неё — обёртка dex.native_wrapped (WETH), а признак «token0 ключа нативный» хранится в
//! состоянии пула (EkuboData.native). Здесь — сопоставление «ключ ↔ пара графа» и направление обмена в порядке КЛЮЧА
//! (пара графа отсортирована по адресу и при нативном token0 может идти в другом порядке, чем ключ). Без сети.
//! Кто вызывает: pools/ekubo_accept.rs (pair), quote/{pool_quote, depth_band, spot}.rs, quote/quoter_ekubo.rs,
//! exec/executor/entry.rs (key_tokens, is_token1).
//! От чего зависит: core/types (EkuboData), discover.rs (sorted).
//! Настройки: нет (обёртку передаёт вызывающий — dex.native_wrapped).
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa: src/math/constants.sol L270 (NATIVE_TOKEN_ADDRESS = 0),
//! src/types/poolKey.sol L11-L18 (token0 < token1 — нативная всегда token0), src/base/FlashAccountant.sol L384-L393
//! (оплата нативной монетой — receive ядра), L348-L356 (выплата нативной — call{value}).

use alloy_primitives::Address;

use crate::core::types::EkuboData;

use super::discover::sorted;

/// Ключ (token0, token1) → пара графа (по адресу) и обёртка вместо нативной (None — нативной нет). Принимает: токены
/// ключа, обёртку (dex.native_wrapped; None — не задана). Ошибки (текст): нативная, а обёртка не задана; ключ — нативная
/// и сама обёртка (пара в графе была бы из одного токена).
pub fn pair(
    t0: Address,
    t1: Address,
    wrapped: Option<Address>,
) -> Result<(Address, Address, Option<Address>), String> {
    if t0 != Address::ZERO {
        let (a, b) = sorted(t0, t1);
        return Ok((a, b, None));
    }
    let Some(w) = wrapped else {
        return Err(
            "token0 пула — нативный ETH (адрес 0), а dex.native_wrapped не задана — сопоставить его не с чем".into(),
        );
    };
    if t1 == w {
        return Err(format!(
            "пул нативный ETH / его обёртка {w:#x} — в графе это один токен, пул не берётся"
        ));
    }
    let (a, b) = sorted(w, t1);
    Ok((a, b, Some(w)))
}

/// Токены ключа пула по паре записи (token0 < token1 по адресу). Ошибок нет.
pub fn key_tokens(d: &EkuboData, token0: Address, token1: Address) -> (Address, Address) {
    match d.native {
        Some(w) => (Address::ZERO, if token0 == w { token1 } else { token0 }),
        None => (token0, token1),
    }
}

/// Вход — token1 КЛЮЧА. Принимает: состояние, token0 записи, токен входа (из пары графа). Ошибок нет.
pub fn is_token1(d: &EkuboData, token0: Address, token_in: Address) -> bool {
    match d.native {
        Some(w) => token_in != w,
        None => token_in != token0,
    }
}
