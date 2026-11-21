//! Файл: venues/bsc_dex/pools/load_crypto_tweak.rs
//! Что делает: решение 472 — чтение состояния `tweak_price` пула Twocrypto-NG / Tricrypto-NG (модель откатов
//! после обмена, math/crypto_tweak2.rs, crypto_tweak3.rs) в той же пачке, что состояние пула (load_crypto.rs read):
//! price_oracle(), totalSupply(), xcp_profit(), virtual_price(), packed_rebalancing_params(). Вызовы идут ПОСЛЕ вызовов load_crypto::calls — разбор
//! прежних ответов не меняется; нет этих ответов (запись до решения 472) — tweak = None (расчёт как раньше).
//! Tricrypto-NG — те же вызовы (price_oracle(k) на каждую монету 1..2). Селекторы totalSupply / xcp_profit /
//! virtual_price / packed_rebalancing_params у обоих видов одинаковы (геттеры без аргументов).
//! Кто вызывает: pools/load_crypto.rs (read, decode).
//! От чего зависит: abi_crypto.rs, core/types (CryptoTweak).
//! Настройки: нет.
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb CurveTwocryptoOptimized.vy — геттеры L151, L154, L158, L193, L1581;
//! `_unpack_3` L716-L726 (allowed_extra_profit = >> 128, adjustment_step = >> 64, по 64 бита); `tweak_price` читает
//! их L833-L840. Tricrypto-NG ecaa816 CurveTricryptoOptimizedWETH.vy — геттеры L171, L174, L179, L219, L1734;
//! `_unpack` L809-L819 (та же раскладка).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::CryptoTweak;

use super::abi_crypto::{ITricryptoPool as P3, ITwocryptoPool as P2};

/// Вызовы состояния tweak_price по порядку: price_oracle (Twocrypto — price_oracle(), Tricrypto — price_oracle(0),
/// price_oracle(1)), totalSupply(), xcp_profit(), virtual_price(), packed_rebalancing_params(). Принимает: пул, число
/// монет (2 или 3; иначе пусто). Ошибок нет.
pub fn calls(pool: Address, n: usize) -> Vec<(Address, Bytes)> {
    let mut data: Vec<Bytes> = match n {
        2 => vec![P2::price_oracleCall {}.abi_encode().into()],
        3 => (0..2u8)
            .map(|k| {
                P3::price_oracleCall { k: U256::from(k) }
                    .abi_encode()
                    .into()
            })
            .collect(),
        _ => return Vec::new(),
    };
    let tail: [Bytes; 4] = [
        P2::totalSupplyCall {}.abi_encode().into(),
        P2::xcp_profitCall {}.abi_encode().into(),
        P2::virtual_priceCall {}.abi_encode().into(),
        P2::packed_rebalancing_paramsCall {}.abi_encode().into(),
    ];
    data.extend(tail);
    data.into_iter().map(|d| (pool, d)).collect()
}

/// Разбор ответов (без сети). Принимает: ответы этих вызовов по порядку (пусто — не читались), число монет.
/// Возвращает: Ok(None) — не читались; Ok(Some) — состояние; Err — откат или ответ короче 32 байт (пул не прочитан:
/// модель нужна, а данных нет).
pub fn decode(got: &[Result<Bytes, String>], n: usize) -> Result<Option<CryptoTweak>, String> {
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
    let o = n.saturating_sub(1);
    let price_oracle = (0..o).map(&w).collect::<Result<Vec<_>, _>>()?;
    let packed = w(o + 3)?;
    let mask = U256::from(u64::MAX);
    Ok(Some(CryptoTweak {
        price_oracle,
        total_supply: w(o)?,
        xcp_profit: w(o + 1)?,
        virtual_price: w(o + 2)?,
        allowed_extra_profit: (packed >> 128usize) & mask,
        adjustment_step: (packed >> 64usize) & mask,
        v1: None,
    }))
}
