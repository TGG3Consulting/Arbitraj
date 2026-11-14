//! Файл: venues/bsc_dex/pools/solidly_gate_read.rs
//! Что делает: решение 273 — ограничения обмена пары Solidly с фабрики (методы без аргументов из настроек DEX):
//! dex.solidly_pause_method (bool; usdfi `isPaused()`), dex.solidly_max_gas_method (uint256 wei; usdfi
//! `maxGasPrice()`); не задан метод — ограничения нет. Вынесено из pools/solidly_fee_read.rs (предел размера файла),
//! переэкспорт оттуда. Ревью 273: предел больше u64 — «предела нет» с записью Info в журнал один раз (п. 5); сбой
//! хранилища настроек в has_gate — ошибка наружу, а не «методы не заданы» (п. 6).
//! Селектор метода из настройки — первые 4 байта keccak256 сигнатуры (solidly_fee_read.rs selector).
//! Кто вызывает: pools/{load_solidly, solidly_refresh}.rs, exec/solidly_gate.rs (через solidly_fee_read.rs).
//! От чего зависит: ctx.rs, node/rpc.rs, pools/solidly_fee_read.rs (optional, selector), core/types (SolidlyGate).
//! Настройки: dex.solidly_pause_method, dex.solidly_max_gas_method [DEX].
//!
//! Факт цепи (14.11.2026, tests/data/solidly_fee_2026-11-14): usdfi BasePair — `require(!factory.isPaused())`,
//! `require(tx.gasprice <= factory.maxGasPrice())` (фабрика verified, solc 0.8.17,
//! https://bscscan.com/address/0xb3863573d9f25e6a84895d4685a408db7a488416#code). eth_call — execution-apis (node/call.rs).

use std::sync::Mutex;

use alloy::sol_types::{SolType, SolValue};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;

use crate::core::types::SolidlyGate;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::families::Family;
use super::solidly_fee_read::{optional, selector};

/// eth_call метода без аргументов по сигнатуре из настройки и разбор ответа как `T` (ABI). Ошибки: узел (откат —
/// с текстом узла), BadResponse (ответ не по ABI).
async fn view_sig<T: SolValue + From<<T::SolType as SolType>::RustType>>(
    ctx: &Ctx,
    to: Address,
    sig: &str,
    tag: &str,
) -> GatewayResult<T> {
    let data = Bytes::copy_from_slice(&selector(sig));
    let out: Bytes = rpc::call(ctx, "eth_call", json!([{ "to": to, "input": data }, tag])).await?;
    T::abi_decode(&out).map_err(|e| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("ответ {sig} у {to:#x} не по ABI: {e}"),
        )
    })
}

/// Ограничения обмена с фабрики (пауза, предел цены газа) — по заданным методам; не задан метод — ограничения нет.
/// Принимает: окружение, вид пула, фабрику, блок. Ошибки: узел, BadResponse (ответ не по ABI).
pub async fn gate(
    ctx: &Ctx,
    f: &Family,
    factory: Address,
    tag: &str,
) -> GatewayResult<SolidlyGate> {
    let mut g = SolidlyGate::default();
    if let Some(sig) = optional(ctx, "dex.solidly_pause_method", f.id)? {
        g.paused = view_sig::<bool>(ctx, factory, &sig, tag).await?;
    }
    if let Some(sig) = optional(ctx, "dex.solidly_max_gas_method", f.id)? {
        let max: U256 = view_sig(ctx, factory, &sig, tag).await?;
        g.max_gas_price = max_gas(max);
        if g.max_gas_price.is_none() && first_wide(f.id, max) {
            let msg = format!(
                "{}: {sig} = {max} не помещается в u64 — предела цены газа нет",
                f.id
            );
            ctx.log(Level::Info, "dex.index", msg, None).await;
        }
    }
    Ok(g)
}

/// Предел цены газа в u64 (ревью 273, п. 5): значение не помещается в u64 (больше u64::MAX wei ≈ 18,4 BNB за единицу
/// газа; например, uint256 max) — «предела нет» (None); помещается — как есть. Иных правил нет. Ошибок нет.
pub fn max_gas(raw: U256) -> Option<u64> {
    u64::try_from(raw).ok()
}

/// Уже записанные в журнал «предел не помещается» (DEX, значение) — Info один раз на пару, а не на каждый пул.
static WIDE: Mutex<Vec<(String, U256)>> = Mutex::new(Vec::new());

/// Первый ли раз (DEX, значение) — запоминает. Ошибок нет.
fn first_wide(id: &str, raw: U256) -> bool {
    let mut seen = WIDE.lock().unwrap_or_else(|p| p.into_inner());
    if seen.iter().any(|(i, v)| i == id && *v == raw) {
        return false;
    }
    seen.push((id.to_string(), raw));
    true
}

/// Заданы ли у DEX методы ограничений обмена (пауза или предел газа). Ошибки: хранилище настроек (кроме «не задана»)
/// — наружу (ревью 273, п. 6: сбой не означает «методы не заданы»).
pub fn has_gate(ctx: &Ctx, f: &Family) -> GatewayResult<bool> {
    for n in ["dex.solidly_pause_method", "dex.solidly_max_gas_method"] {
        if optional(ctx, n, f.id)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}
