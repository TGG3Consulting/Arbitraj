//! Файл: venues/bsc_dex/pools/solidly_fee_read.rs
//! Что делает: решение 273 — комиссия пула Solidly по источнику из настроек DEX и ограничения обмена пары с фабрики.
//! Источник dex.solidly_fee_source (не задан — источник по виду пула, как до решения 273: load_solidly.rs):
//! - factory_pool — `getFee(address pool, bool stable)` фабрики (Velodrome PoolFactory.sol L131-L133);
//! - factory_stable — `getFee(bool stable)` фабрики (THENA V1 PairFactoryUpgradeable.sol L104-L106);
//! - pool — view-метод пула без аргументов (dex.solidly_fee_method; usdfi `fee()`, veplus `swapFee()`);
//! - factory_by_type — view-метод фабрики без аргументов по флагу stable пула (dex.solidly_fee_method_stable /
//!   dex.solidly_fee_method_volatile; magicfox `stableFee()` / `volatileFee()`).
//!
//! Правило применения — dex.solidly_fee_rule (numerator: над dex.solidly_fee_denominator | divisor).
//! Ограничения обмена (фабрика, методы без аргументов): dex.solidly_pause_method (bool; usdfi `isPaused()`),
//! dex.solidly_max_gas_method (uint256 wei; usdfi `maxGasPrice()`); не заданы — ограничения нет.
//! Селектор метода из настройки — первые 4 байта keccak256 сигнатуры (как dex.v2_fee_method, pools/verify_calls.rs).
//! Ревью 273: fee_call / rule — вызов и правило комиссии отдельно (пачка pools/solidly_refresh.rs); ограничения обмена
//! (gate, has_gate, max_gas) — pools/solidly_gate_read.rs, переэкспорт отсюда.
//! Решение 325: fee сначала смотрит в книгу заранее прочитанных ответов задачи (node/prefetch.rs; книги нет — как раньше).
//! Кто вызывает: pools/load_solidly.rs, pools/solidly_refresh.rs, exec/solidly_gate.rs.
//! От чего зависит: ctx.rs, node/rpc.rs, pools/abi_solidly.rs, families.rs, math/solidly_math.rs (fee_denominator),
//! core/types (SolidlyFee, SolidlyGate).
//! Настройки: dex.solidly_fee_source, dex.solidly_fee_method, dex.solidly_fee_method_stable,
//! dex.solidly_fee_method_volatile, dex.solidly_fee_rule, dex.solidly_fee_denominator, dex.solidly_pause_method,
//! dex.solidly_max_gas_method [DEX].
//!
//! Факты цепи (14.11.2026, tests/data/solidly_fee_2026-11-14): usdfi — фабрика BaseFactory (verified, solc 0.8.17,
//! https://bscscan.com/address/0xb3863573d9f25e6a84895d4685a408db7a488416#code), getFee(bool) и getFee(address,bool)
//! откатываются; пара BasePair: `amountIn -= amountIn / fee`, swap требует `!factory.isPaused()` и
//! `tx.gasprice <= factory.maxGasPrice()`. eth_call — execution-apis (ссылка в node/call.rs).

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::json;

use crate::core::types::{FeeRule, SolidlyFee};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::solidly_math::fee_denominator;
use super::super::node::rpc;
use super::abi_solidly::{ISolidlyFactory, IThenaV1Factory};
use super::families::{Family, Proto};

/// Ограничения обмена пары (пауза, предел цены газа) — pools/solidly_gate_read.rs (вынесено: предел размера файла).
pub use super::solidly_gate_read::{gate, has_gate, max_gas};

/// Необязательная настройка DEX: не задана — None; иные ошибки хранилища — ошибка.
pub(super) fn optional(ctx: &Ctx, name: &str, id: &str) -> GatewayResult<Option<String>> {
    match ctx.dex_text(name, id) {
        Ok(s) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Ok(_) => Ok(None),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e),
    }
}

/// Источник комиссии из dex.solidly_fee_source. None — не задан (по виду пула, как до решения 273). Ошибки: хранилище.
pub fn source(ctx: &Ctx, f: &Family) -> GatewayResult<Option<String>> {
    optional(ctx, "dex.solidly_fee_source", f.id)
}

/// Комиссия пула по заданному источнику. Принимает: окружение, вид пула, источник, фабрику, пул, флаг stable, блок.
/// Возвращает: значение и правило. Ошибки: NotConfigured (метод, правило, делитель; неизвестный источник), узел,
/// BadResponse (значение не помещается или комиссия ≥ 100% по правилу).
pub async fn fee(
    ctx: &Ctx,
    f: &Family,
    src: &str,
    (factory, pool, stable): (Address, Address, bool),
    tag: &str,
) -> GatewayResult<SolidlyFee> {
    let rule = rule(ctx, f, Some(src))?;
    let (to, data) = fee_call(ctx, f, Some(src), (factory, pool, stable))?;
    // Решение 325: ответ, прочитанный заранее на этом блоке (node/prefetch.rs), — без обращения к узлу.
    let out: Bytes = match super::super::node::prefetch::hit(to, &data, tag) {
        Some(b) => b,
        None => rpc::call(ctx, "eth_call", json!([{ "to": to, "input": data }, tag])).await?,
    };
    checked(ctx, word(ctx, to, &out)?, rule)
}

/// Вызов комиссии пула (контракт, данные) — для одного eth_call и для пачки (pools/solidly_refresh.rs). Источник
/// задан — по нему (шапка); None — по виду пула, как до решения 273 (load_solidly.rs pool_fee: getFee(stable) у
/// Solidly V1, getFee(pool, stable) у Solidly). Ошибки: NotConfigured (метод источника; неизвестный источник).
pub fn fee_call(
    ctx: &Ctx,
    f: &Family,
    src: Option<&str>,
    (factory, pool, stable): (Address, Address, bool),
) -> GatewayResult<(Address, Bytes)> {
    let need = |name: &str| {
        optional(ctx, name, f.id)?
            .ok_or_else(|| ctx.not_set(name, format!("{}: не задана {name}", f.id)))
    };
    let sig = |s: String| Bytes::copy_from_slice(&selector(&s));
    let v1 = || Bytes::from(IThenaV1Factory::getFeeCall { stable }.abi_encode());
    let by_pool = || Bytes::from(ISolidlyFactory::getFeeCall { pool, stable }.abi_encode());
    Ok(match src {
        None if f.proto == Proto::SolidlyV1 => (factory, v1()),
        None | Some("factory_pool") => (factory, by_pool()),
        Some("factory_stable") => (factory, v1()),
        Some("factory_address") => {
            let signature = need("dex.solidly_fee_method")?;
            let mut data = selector(&signature).to_vec();
            data.extend_from_slice(&pool.abi_encode());
            (factory, data.into())
        }
        Some("pool") => (pool, sig(need("dex.solidly_fee_method")?)),
        Some("factory_by_type") => {
            let name = if stable {
                "dex.solidly_fee_method_stable"
            } else {
                "dex.solidly_fee_method_volatile"
            };
            (factory, sig(need(name)?))
        }
        Some(other) => {
            let why = format!("{}: dex.solidly_fee_source = «{other}» неизвестен", f.id);
            return Err(ctx.not_set("dex.solidly_fee_source", why));
        }
    })
}

/// Правило комиссии: источник задан — dex.solidly_fee_rule (numerator — над dex.solidly_fee_denominator | divisor);
/// None — «числитель над fee_denominator()», как до решения 273. Ошибки: NotConfigured (правило, делитель).
pub fn rule(ctx: &Ctx, f: &Family, src: Option<&str>) -> GatewayResult<FeeRule> {
    if src.is_none() {
        let denominator = fee_denominator().to::<u64>();
        return Ok(FeeRule::Numerator { denominator });
    }
    let name = "dex.solidly_fee_rule";
    let set = optional(ctx, name, f.id)?
        .ok_or_else(|| ctx.not_set(name, format!("{}: не задана {name}", f.id)))?;
    match set.as_str() {
        "numerator" => {
            let denominator = ctx.dex_int("dex.solidly_fee_denominator", f.id)?;
            if f.variant != super::family_variant::Variant::SolidlyClassic
                && denominator > u64::from(u32::MAX)
            {
                return Err(ctx.not_set("dex.solidly_fee_denominator", "делитель не помещается"));
            }
            Ok(FeeRule::Numerator { denominator })
        }
        "divisor" => Ok(FeeRule::Divisor),
        other => {
            let why = format!("{}: dex.solidly_fee_rule = «{other}» неизвестно", f.id);
            Err(ctx.not_set(name, why))
        }
    }
}

/// Ответ метода комиссии (uint256) по ABI. Принимает: контракт (для текста), байты. Ошибка BadResponse — не по ABI.
pub fn word(ctx: &Ctx, to: Address, out: &[u8]) -> GatewayResult<U256> {
    U256::abi_decode(out).map_err(|e| {
        let why = format!("ответ комиссии Solidly у {to:#x} не по ABI: {e}");
        ctx.err(ErrorCode::BadResponse, why)
    })
}

/// Число комиссии в u64 и проверка по правилу: числитель — меньше делителя; делитель — больше 1 (1 — комиссия 100%,
/// 0 — деление на ноль в контракте). Ошибка BadResponse — иначе.
pub fn checked(ctx: &Ctx, raw: U256, rule: FeeRule) -> GatewayResult<SolidlyFee> {
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, w);
    let value: u64 = raw
        .try_into()
        .map_err(|_| bad(format!("комиссия Solidly {raw} не помещается")))?;
    let ok = match rule {
        FeeRule::Numerator { denominator } => value < denominator,
        FeeRule::Divisor => (2..=u64::from(u32::MAX)).contains(&value),
    };
    if !ok {
        return Err(bad(format!(
            "комиссия Solidly {value} по правилу {rule:?} — 100% и больше (пул испорчен)"
        )));
    }
    Ok(SolidlyFee { value, rule })
}

/// Селектор вызова (для журнала и тестов): первые 4 байта keccak256 сигнатуры. Ошибок нет.
pub fn selector(sig: &str) -> [u8; 4] {
    let h = keccak256(sig.as_bytes());
    [h[0], h[1], h[2], h[3]]
}
