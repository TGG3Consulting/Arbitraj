//! Файл: venues/bsc_dex/pools/verify_rules.rs
//! Что делает: решения проверки пула V2/V3 по прочитанному с узла — без сети (решение 58): разбор ответов полей пула,
//! сверка фабрики и токенов с настройками и монетой, ответ реестра фабрики, своя комиссия пары V2 в миллионных долях.
//! Кто вызывает: pools/verify.rs; tests/bsc_dex_index.rs.
//! От чего зависит: abi_check.rs, abi_v2v3.rs, discover.rs (sorted), families.rs, verify.rs (Candidate).
//! Настройки: нет (значения передаёт вызывающий).
//!
//! ABI и ссылки на официальные исходники — abi_check.rs, abi_v2v3.rs.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::abi_check::IPoolImmutables as P;
use super::abi_v2v3::IV2Factory;
use super::discover::sorted;
use super::families::{Family, Proto};
use super::verify::Candidate;

/// Прочитанное с пула V2/V3.
#[derive(Debug, Clone, Default)]
pub struct Read {
    pub factory: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: Option<u32>,
    pub spacing: Option<i32>,
    /// V2 со своей комиссией: значение dex.v2_fee_method.
    pub v2_fee: Option<U256>,
}

/// Разбор ответов полей пула (порядок — field_calls). Ошибка — откат или не тот ответ: это не пул этого вида.
pub fn fill(f: &Family, r: &mut Read, got: &[Result<Bytes, String>]) -> Result<(), String> {
    let bytes = |i: usize, what: &str| -> Result<&Bytes, String> {
        got.get(i)
            .ok_or_else(|| format!("нет ответа {what}"))?
            .as_ref()
            .map_err(|w| format!("{what}: {w} — это не пул {}", f.title))
    };
    let dec = |e: alloy::sol_types::Error| format!("ответ не по ABI: {e}");
    r.factory = P::factoryCall::abi_decode_returns(bytes(0, "factory()")?).map_err(dec)?;
    r.token0 = P::token0Call::abi_decode_returns(bytes(1, "token0()")?).map_err(dec)?;
    r.token1 = P::token1Call::abi_decode_returns(bytes(2, "token1()")?).map_err(dec)?;
    match f.proto {
        Proto::V2 if got.len() > 3 => {
            let b = bytes(3, "метод комиссии пары")?;
            if b.len() < 32 {
                return Err("метод комиссии пары вернул не число".into());
            }
            r.v2_fee = Some(U256::from_be_slice(&b[..32]));
        }
        Proto::V2 => {}
        _ => {
            r.fee = Some(
                P::feeCall::abi_decode_returns(bytes(3, "fee()")?)
                    .map_err(dec)?
                    .to(),
            );
            let s =
                P::tickSpacingCall::abi_decode_returns(bytes(4, "tickSpacing()")?).map_err(dec)?;
            r.spacing = Some(s.as_i32());
        }
    }
    Ok(())
}

/// Сверка полей с настройками и монетой (без сети). Принимает: кандидата, фабрику DEX из настроек, прочитанное.
/// Ошибка — причина отказа.
pub fn check(c: &Candidate, expected_factory: Address, r: &Read) -> Result<(), String> {
    if r.factory != expected_factory {
        return Err(format!(
            "factory() пула = {}, у DEX {} фабрика {expected_factory} (dex.factory)",
            r.factory, c.family.id
        ));
    }
    let want = sorted(c.coin, c.quote);
    if (r.token0, r.token1) != want {
        return Err(format!(
            "токены пула {} / {} ≠ монета {} и встречный {} (адрес монеты — от биржи)",
            r.token0, r.token1, c.coin, c.quote
        ));
    }
    Ok(())
}

/// Реестр фабрики вернул этот же пул. Принимает: адрес пула, ответ реестра. Ошибка — причина отказа.
pub fn registered(pool: Address, answer: &[u8]) -> Result<(), String> {
    let got = IV2Factory::getPairCall::abi_decode_returns(answer)
        .map_err(|e| format!("ответ реестра фабрики не по ABI: {e}"))?;
    if got == pool {
        Ok(())
    } else {
        Err(format!(
            "фабрика DEX не знает этот пул: её реестр по этим токенам даёт {got}"
        ))
    }
}

/// Комиссия пары V2 в миллионных долях: значение метода × 1 000 000 / делитель. Ошибка — не меньше 100%.
pub fn v2_fee_pips(value: U256, denominator: u64) -> Result<u32, String> {
    let pips = value.saturating_mul(U256::from(1_000_000u32)) / U256::from(denominator.max(1));
    match u32::try_from(pips) {
        Ok(p) if p < 1_000_000 => Ok(p),
        _ => Err(format!(
            "комиссия пары {value}/{denominator} — не меньше 100%"
        )),
    }
}
