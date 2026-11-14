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
//! Кто вызывает: pools/load_solidly.rs, pools/solidly_refresh.rs, exec/solidly_gate.rs.
//! От чего зависит: ctx.rs, node/rpc.rs, pools/abi_solidly.rs, families.rs, core/types (SolidlyFee, SolidlyGate).
//! Настройки: dex.solidly_fee_source, dex.solidly_fee_method, dex.solidly_fee_method_stable,
//! dex.solidly_fee_method_volatile, dex.solidly_fee_rule, dex.solidly_fee_denominator, dex.solidly_pause_method,
//! dex.solidly_max_gas_method [DEX].
//!
//! Факты цепи (14.11.2026, tests/data/solidly_fee_2026-11-14): usdfi — фабрика BaseFactory (verified, solc 0.8.17,
//! https://bscscan.com/address/0xb3863573d9f25e6a84895d4685a408db7a488416#code), getFee(bool) и getFee(address,bool)
//! откатываются; пара BasePair: `amountIn -= amountIn / fee`, swap требует `!factory.isPaused()` и
//! `tx.gasprice <= factory.maxGasPrice()`. eth_call — execution-apis (ссылка в node/call.rs).

use alloy::sol_types::{SolType, SolValue};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::json;

use crate::core::types::{FeeRule, SolidlyFee, SolidlyGate};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::abi_solidly::{ISolidlyFactory, IThenaV1Factory};
use super::families::Family;

/// Необязательная настройка DEX: не задана — None; иные ошибки хранилища — ошибка.
fn optional(ctx: &Ctx, name: &str, id: &str) -> GatewayResult<Option<String>> {
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

/// eth_call метода без аргументов по сигнатуре из настройки и разбор ответа как `T` (ABI). Ошибки: узел (откат —
/// с текстом узла), BadResponse (ответ не по ABI).
async fn view_sig<T: SolValue + From<<T::SolType as SolType>::RustType>>(
    ctx: &Ctx,
    to: Address,
    sig: &str,
    tag: &str,
) -> GatewayResult<T> {
    let data = Bytes::copy_from_slice(&keccak256(sig.as_bytes())[..4]);
    let out: Bytes = rpc::call(ctx, "eth_call", json!([{ "to": to, "input": data }, tag])).await?;
    T::abi_decode(&out).map_err(|e| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("ответ {sig} у {to:#x} не по ABI: {e}"),
        )
    })
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
    let need = |name: &str| {
        optional(ctx, name, f.id)?.ok_or_else(|| ctx.not_set(name, format!("{}: не задана {name}", f.id)))
    };
    let raw: U256 = match src {
        "factory_pool" => {
            let c = ISolidlyFactory::getFeeCall { pool, stable };
            super::super::node::call::view(ctx, factory, &c, tag).await?
        }
        "factory_stable" => {
            let c = IThenaV1Factory::getFeeCall { stable };
            super::super::node::call::view(ctx, factory, &c, tag).await?
        }
        "pool" => view_sig(ctx, pool, &need("dex.solidly_fee_method")?, tag).await?,
        "factory_by_type" => {
            let name = if stable {
                "dex.solidly_fee_method_stable"
            } else {
                "dex.solidly_fee_method_volatile"
            };
            view_sig(ctx, factory, &need(name)?, tag).await?
        }
        other => {
            let why = format!("{}: dex.solidly_fee_source = «{other}» неизвестен", f.id);
            return Err(ctx.not_set("dex.solidly_fee_source", why));
        }
    };
    let rule = match need("dex.solidly_fee_rule")?.as_str() {
        "numerator" => FeeRule::Numerator {
            denominator: u32::try_from(ctx.dex_int("dex.solidly_fee_denominator", f.id)?)
                .map_err(|_| ctx.not_set("dex.solidly_fee_denominator", "делитель не помещается"))?,
        },
        "divisor" => FeeRule::Divisor,
        other => {
            let why = format!("{}: dex.solidly_fee_rule = «{other}» неизвестно", f.id);
            return Err(ctx.not_set("dex.solidly_fee_rule", why));
        }
    };
    checked(ctx, raw, rule)
}

/// Число комиссии в u32 и проверка по правилу: числитель — меньше делителя; делитель — больше 1 (1 — комиссия 100%,
/// 0 — деление на ноль в контракте). Ошибка BadResponse — иначе.
pub fn checked(ctx: &Ctx, raw: U256, rule: FeeRule) -> GatewayResult<SolidlyFee> {
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, w);
    let value: u32 = raw
        .try_into()
        .map_err(|_| bad(format!("комиссия Solidly {raw} не помещается")))?;
    let ok = match rule {
        FeeRule::Numerator { denominator } => value < denominator,
        FeeRule::Divisor => value > 1,
    };
    if !ok {
        return Err(bad(format!(
            "комиссия Solidly {value} по правилу {rule:?} — 100% и больше (пул испорчен)"
        )));
    }
    Ok(SolidlyFee { value, rule })
}

/// Ограничения обмена с фабрики (пауза, предел цены газа) — по заданным методам; не задан метод — ограничения нет.
/// Принимает: окружение, вид пула, фабрику, блок. Ошибки: узел, BadResponse (ответ не по ABI, предел больше u64).
pub async fn gate(ctx: &Ctx, f: &Family, factory: Address, tag: &str) -> GatewayResult<SolidlyGate> {
    let mut g = SolidlyGate::default();
    if let Some(sig) = optional(ctx, "dex.solidly_pause_method", f.id)? {
        g.paused = view_sig::<bool>(ctx, factory, &sig, tag).await?;
    }
    if let Some(sig) = optional(ctx, "dex.solidly_max_gas_method", f.id)? {
        let max: U256 = view_sig(ctx, factory, &sig, tag).await?;
        g.max_gas_price = Some(u64::try_from(max).map_err(|_| {
            ctx.err(ErrorCode::BadResponse, format!("{sig} = {max} не помещается в u64"))
        })?);
    }
    Ok(g)
}

/// Заданы ли у DEX методы ограничений обмена (пауза или предел газа). Ошибок нет (сбой хранилища — «не заданы»).
pub fn has_gate(ctx: &Ctx, f: &Family) -> bool {
    ["dex.solidly_pause_method", "dex.solidly_max_gas_method"]
        .iter()
        .any(|n| optional(ctx, n, f.id).ok().flatten().is_some())
}

/// Селектор вызова (для журнала и тестов): первые 4 байта keccak256 сигнатуры. Ошибок нет.
pub fn selector(sig: &str) -> [u8; 4] {
    let h = keccak256(sig.as_bytes());
    [h[0], h[1], h[2], h[3]]
}
