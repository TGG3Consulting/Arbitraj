//! Файл: venues/bsc_dex/pools/load_crypto_tweak_list_decode.rs
//! Что делает: решение 472 (T472-03) — разбор (без сети) ответов чтения tweak_price пулов Curve первого поколения без
//! фабрики (load_crypto_tweak_list.rs calls): оракул на время блока чтения (math/crypto_math3.rs oracle_at; tricrypto —
//! упаковка `assert p < PRICE_MASK`, «2021-09-13. Arbitrum/CryptoSwap.vy» L460-L466), состояние CryptoTweak с частью v1.
//! Вынесено из load_crypto_tweak_list.rs (предел размера файла; путь — pub use там).
//! Кто вызывает: pools/load_crypto_tweak_list.rs (реэкспорт), load_crypto_v1.rs (state), тесты на записи.
//! От чего зависит: core/types (CryptoTweak, CryptoTweakV1), math/crypto_math3.rs, math/crypto_tweak_v1_3.rs.
//! Настройки: нет.

use alloy_primitives::{Bytes, U256};

use crate::core::types::pool_crypto::{CryptoTweak, CryptoTweakV1};

use super::super::math::crypto_math3::oracle_at;
use super::super::math::crypto_tweak_v1_3::price_mask;

/// Разбор (без сети). Принимает: ответы вызовов tweak и последним — слово слота not_adjusted (пусто — не читались),
/// число монет, время блока чтения. Возвращает: Ok(None) — не читались; Ok(Some) — состояние; Err — откат вызова.
pub fn decode(
    got: &[Result<Bytes, String>],
    n: usize,
    now: u64,
) -> Result<Option<CryptoTweak>, String> {
    if got.is_empty() {
        return Ok(None);
    }
    let w = |i: usize| -> Result<U256, String> {
        match got.get(i) {
            Some(Ok(b)) if b.len() >= 32 => Ok(U256::from_be_slice(&b[..32])),
            Some(Ok(_)) => Err(format!("ответ tweak {i} короче 32 байт")),
            Some(Err(r)) => Err(format!("вызов tweak {i} откатился: {r}")),
            None => Err(format!("нет ответа tweak {i}")),
        }
    };
    let now = U256::from(now);
    // Индексы: (last_prices, lpt, mht, oracle, xcp_profit, virtual_price, aep, step, totalSupply, слово слота).
    let (last, lpt, mht, ora): (Vec<U256>, U256, U256, Vec<U256>) = if n == 3 {
        (vec![w(0)?, w(1)?], w(2)?, w(3)?, vec![w(4)?, w(5)?])
    } else {
        (vec![w(0)?], w(1)?, w(2)?, vec![w(3)?])
    };
    // Оракул на время блока чтения; откат пересчёта или упаковки (tricrypto) — пусто: модель отказывает.
    let mut oracle = Vec::with_capacity(ora.len());
    for (l, o) in last.iter().zip(&ora) {
        match oracle_at(*l, *o, lpt, mht, now) {
            Some(x) if n == 2 || lpt >= now || x < price_mask() => oracle.push(x),
            _ => {
                oracle.clear();
                break;
            }
        }
    }
    let (xp, vp, aep, step, ts) = if n == 3 {
        (6, 7, 8, 9, 10)
    } else {
        (4, 6, 7, 8, 11)
    };
    let gulp = if n == 3 {
        Vec::new()
    } else {
        vec![w(12)?, w(13)?]
    };
    let slot_word = if n == 3 { 11 } else { 14 };
    Ok(Some(CryptoTweak {
        price_oracle: oracle,
        total_supply: w(ts)?,
        xcp_profit: w(xp)?,
        virtual_price: w(vp)?,
        allowed_extra_profit: w(aep)?,
        adjustment_step: w(step)?,
        v1: Some(CryptoTweakV1 {
            not_adjusted: !w(slot_word)?.is_zero(),
            xcp_profit_a: if n == 3 { U256::ZERO } else { w(5)? },
            admin_fee: if n == 3 { U256::ZERO } else { w(9)? },
            fee_receiver: n == 2 && !w(10)?.is_zero(),
            gulp,
            factory: false,
            last_prices: if n == 3 { last } else { Vec::new() },
        }),
    }))
}
