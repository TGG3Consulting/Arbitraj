//! Файл: venues/bsc_dex/pools/verify_rules.rs
//! Что делает: решения проверки пула V2/V3 по прочитанному с узла — без сети (решение 58): разбор ответов полей пула,
//! сверка фабрики и токенов с настройками и монетой, ABI V3 (PancakeSwap или Uniswap — по lmPool()), ответ реестра фабрики, своя комиссия пары V2 в миллионных долях.
//! Кто вызывает: pools/verify.rs; tests/bsc_dex_index.rs.
//! От чего зависит: abi_check.rs, abi_v2v3.rs, discover.rs (sorted), families.rs, verify.rs (Candidate).
//! Настройки: нет (значения передаёт вызывающий).
//!
//! ABI и ссылки на официальные исходники — abi_check.rs, abi_v2v3.rs.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::abi_check::IPoolImmutables as P;
use super::abi_elastic::IElasticPool;
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
        // KyberSwap Elastic (решение 81): swapFeeUnits → fee (feeUnits), tickDistance → spacing. Нет lmPool (не V3).
        Proto::KyberElastic => {
            r.fee = Some(
                IElasticPool::swapFeeUnitsCall::abi_decode_returns(bytes(3, "swapFeeUnits()")?)
                    .map_err(dec)?
                    .to(),
            );
            let d = IElasticPool::tickDistanceCall::abi_decode_returns(bytes(4, "tickDistance()")?)
                .map_err(dec)?;
            r.spacing = Some(d.as_i32());
        }
        _ => {
            r.fee = Some(
                P::feeCall::abi_decode_returns(bytes(3, "fee()")?)
                    .map_err(dec)?
                    .to(),
            );
            let s =
                P::tickSpacingCall::abi_decode_returns(bytes(4, "tickSpacing()")?).map_err(dec)?;
            r.spacing = Some(s.as_i32());
            v3_abi(f, got.get(5))?;
        }
    }
    Ok(())
}

/// ABI V3 по узлу: у пула PancakeSwap V3 есть lmPool() (PancakeV3Pool.sol#L106, pancake-v3-contracts 98684794),
/// у Uniswap V3 его нет (UniswapV3Pool.sol без fallback — вызов откатывается). Вид DEX должен совпасть с пулом:
/// разбор событий и slot0 у них разный. Принимает: вид, ответ lmPool(). Ошибка — причина отказа.
pub fn v3_abi(f: &Family, lm: Option<&Result<Bytes, String>>) -> Result<(), String> {
    let has = matches!(lm, Some(Ok(b)) if b.len() >= 32);
    match (f.proto, has) {
        (Proto::PancakeV3, false) => Err(format!(
            "у пула нет lmPool() — это не пул PancakeSwap V3, а вид DEX {} — PancakeSwap V3",
            f.id
        )),
        (Proto::UniswapV3, true) => Err(format!(
            "пул отвечает lmPool() — это пул PancakeSwap V3, а вид DEX {} — Uniswap V3",
            f.id
        )),
        _ => Ok(()),
    }
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

/// Комиссия пары V2 в миллионных долях: значение метода × 1 000 000 / делитель. Ошибка — делитель 0, не меньше 100%.
pub fn v2_fee_pips(value: U256, denominator: u64) -> Result<u32, String> {
    if denominator == 0 {
        return Err("делитель комиссии пары dex.v2_fee_denominator = 0".into());
    }
    let pips = value.saturating_mul(U256::from(1_000_000u32)) / U256::from(denominator);
    match u32::try_from(pips) {
        Ok(p) if p < 1_000_000 => Ok(p),
        _ => Err(format!(
            "комиссия пары {value}/{denominator} — не меньше 100%"
        )),
    }
}
