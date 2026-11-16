//! Файл: venues/bsc_dex/pools/integral_fee.rs
//! Что делает: решение 325 — у пула Algebra Integral перечитывается ТОЛЬКО комиссия, когда событие меняет только её
//! (Applied::Fee, events_algebra_integral.rs): Fee(uint16) — новая lastFee; SwapFee и Swap 1.2.0/1.2.1 с overrideFee = 0
//! и pluginFee ≠ 0 — цена, ликвидность и тик уже применены из события, неизвестна только комиссия. Раньше в этих
//! случаях пул перечитывался целиком (голова и обход списка тиков: 4–5 + до 2 × dex.integral_tick_steps обращений
//! подряд). Здесь — globalState и fee() одним обращением (node/call.rs try_many: Multicall3.aggregate3 при
//! dex.multicall3, иначе пачка JSON-RPC одним HTTP-запросом) на блоке события; комиссия — по тому же правилу, что у
//! полного чтения (load_algebra_integral.rs: флаг DYNAMIC_FEE в pluginConfig — fee() пула, иначе lastFee; правило —
//! одна функция `dynamic` на оба места).
//! Включаемый: dex.event_refresh_fee_only = да для DEX пула (область «DEX»). Нет / не задана — Applied::Fee превращается
//! в прежний Refresh (полное перечитывание с той же записью журнала), 1 в 1 как раньше.
//! Кто вызывает: pools/listen_log.rs (Applied::Fee), pools/load_algebra_integral.rs (dynamic).
//! От чего зависит: abi_algebra_integral.rs, node/call.rs (try_many, decode, block_tag), registry.rs, listen.rs (Pools).
//! Настройки: dex.event_refresh_fee_only [DEX]; dex.multicall3 [bsc_dex] (через try_many).
//!
//! ABI: integral-core IAlgebraPoolState.sol L44 globalState() (… lastFee, pluginConfig …), L117 fee() (при DYNAMIC_FEE —
//! getCurrentFee() плагина, AlgebraPoolBase.sol L145-L150); Plugins.sol L26 DYNAMIC_FEE (abi_algebra_integral.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use chrono::Utc;

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::{self, block_tag};
use super::abi_algebra_integral::{DYNAMIC_FEE, IAlgebraIntegralPool as P};
use super::families::Family;
use super::listen::Pools;

/// Комиссию определяет fee() пула (флаг DYNAMIC_FEE в pluginConfig), а не lastFee. Ошибок нет.
pub fn dynamic(plugin_config: u8) -> bool {
    plugin_config & DYNAMIC_FEE != 0
}

/// Включено ли «только комиссия» для DEX пула. Ошибок нет: не задано — false.
pub fn on(ctx: &Ctx, f: &Family) -> bool {
    ctx.dex_flag("dex.event_refresh_fee_only", f.id)
        .unwrap_or(false)
}

/// Комиссия пула на блоке одним обращением. Принимает: окружение, пул, блок-тег.
/// Возвращает: комиссию (пипсы, 1e-6). Ошибки: узел; BadResponse — откат нужного вызова, ответ не по ABI.
pub async fn read(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<u32> {
    let calls = [
        (pool, Bytes::from(P::globalStateCall {}.abi_encode())),
        (pool, Bytes::from(P::feeCall {}.abi_encode())),
    ];
    let got = call::try_many(ctx, &calls, tag).await?;
    let pick = |i: usize| -> GatewayResult<Bytes> {
        match got.get(i) {
            Some(Ok(b)) => Ok(b.clone()),
            Some(Err(why)) => Err(ctx.err(
                ErrorCode::BadResponse,
                format!("Algebra Integral {pool}: {why}"),
            )),
            None => Err(ctx.err(
                ErrorCode::BadResponse,
                format!("Algebra Integral {pool}: ответов меньше, чем вызовов"),
            )),
        }
    };
    let gs = call::decode::<P::globalStateCall>(&pick(0)?)?;
    if dynamic(gs.pluginConfig) {
        return Ok(u32::from(call::decode::<P::feeCall>(&pick(1)?)?));
    }
    Ok(u32::from(gs.lastFee))
}

/// Перечитывает только комиссию пула на блоке события и отмечает событие учтённым (как Applied::Updated в
/// events.rs: блок, позиция лога). Принимает: окружение пачки, ссылку, блок и logIndex события.
/// Ошибки: как у read; Internal — ссылка не на свой контракт.
pub async fn apply(p: &Pools, r: &PoolRef, block: u64, index: Option<u64>) -> GatewayResult<()> {
    let PoolRef::Contract(pool) = r else {
        return Err(p.ctx.err(
            ErrorCode::Internal,
            "Algebra Integral: ссылка на пул не того вида",
        ));
    };
    let fee = read(&p.ctx, *pool, &block_tag(block)).await?;
    p.reg.update(r, |e| {
        if let PoolState::Algebra { fee: f, .. } = &mut e.pool.state {
            *f = fee;
        }
        e.pool.block = block;
        e.synced_block = block;
        e.synced_log = index;
        e.pool.received_at = Utc::now();
    });
    Ok(())
}
