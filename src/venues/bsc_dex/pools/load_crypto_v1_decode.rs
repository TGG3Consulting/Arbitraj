//! Файл: venues/bsc_dex/pools/load_crypto_v1_decode.rs
//! Что делает: разбор ответов чтения crypto-пула Curve первого поколения (load_crypto_v1.rs calls_with) без сети —
//! монеты, балансы, множители точности (10^(18 − decimals)), price_scale, D, A, gamma, комиссии, рампа, is_killed.
//! Вынесено из load_crypto_v1.rs (предел размера файла; решение 472, ревью); путь `load_crypto_v1::decode` — прежний
//! (pub use).
//! Кто вызывает: pools/load_crypto_v1.rs (state), тесты на записи.
//! От чего зависит: load_crypto_v1.rs (Raw, HEAD), abi_crypto.rs, math/ten.rs, core/types.
//! Настройки: нет.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};

use crate::core::types::{CryptoData, CryptoMath};

use super::super::math::ten;
use super::abi_crypto::ICryptoPool as P;
use super::load_crypto_v1::{HEAD, Raw};

/// Что делает: разбор ответов (без сети). Принимает: Raw. Возвращает: (монеты, CryptoData с индексами пары 0) или текст
/// причины (откат вызова, ответ не по ABI, decimals > 18).
pub fn decode(raw: &Raw) -> Result<(Vec<Address>, CryptoData), String> {
    let got = &raw.got;
    let ok = |i: usize| match got.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!("вызов {i} откатился: {r}")),
        None => Err(format!("нет ответа на вызов {i}")),
    };
    let w = |i: usize| -> Result<U256, String> {
        let v = ok(i)?.get(..32).ok_or(format!("ответ {i} короче 32 байт"))?;
        Ok(U256::from_be_slice(v))
    };
    let mut coins = Vec::with_capacity(raw.n);
    let mut balances = Vec::with_capacity(raw.n);
    let mut precisions = Vec::with_capacity(raw.n);
    for k in 0..raw.n {
        let c = P::coinsCall::abi_decode_returns(ok(HEAD + 2 * k)?).map_err(|e| e.to_string())?;
        coins.push(c);
        balances.push(w(HEAD + 2 * k + 1)?);
        let d = *raw.decimals.get(k).ok_or("нет decimals монеты")?;
        let p = 18u64.checked_sub(d.into()).ok_or(format!("decimals {d} > 18"))?;
        precisions.push(ten::pow(p));
    }
    let tail = HEAD + 2 * raw.n;
    let price_scale = (0..raw.n - 1)
        .map(|k| w(tail + k))
        .collect::<Result<Vec<_>, _>>()?;
    // is_killed() — у tricrypto 2021 (exchange: assert not self.is_killed, CryptoSwap.vy L594) и пары без фабрики
    // (решение 472: EURS CryptoSwap.vy L688).
    let halted = raw.killed && !w(tail + raw.n - 1)?.is_zero();
    let t = w(6)?;
    let data = CryptoData {
        balances,
        precisions,
        price_scale,
        d: w(2)?,
        a: w(0)?,
        gamma: w(1)?,
        mid_fee: w(3)?,
        out_fee: w(4)?,
        fee_gamma: w(5)?,
        math: if raw.n == 2 { CryptoMath::TwoV1 } else { CryptoMath::TriV1 },
        ramp: !t.is_zero(),
        moving: t > U256::from(raw.now),
        halted,
        native_in: false,
        i0: 0,
        i1: 0,
        tweak: None,
    };
    Ok((coins, data))
}
