//! Файл: venues/bsc_dex/pools/aero_cl.rs
//! Что делает: решение 470 (T451-01) — комиссия и проверки пула Aero «Slipstream V3» (вид slipstream_aero: тот же
//! CLPool, что у вида slipstream, но комиссия обмена — от крючка). На блоке: крючок пула у фабрики
//! (getPoolSwapHook) равен заданному dex.cl_swap_hook (иначе пул не читается: поведение чужого крючка не сверено);
//! правило MEV-налога модуля крючка (aero_mev.rs); комиссия — наибольшая из fee() пула на этом блоке и первой комиссии
//! следующего блока (T470-01: порт _getFirstSwapFee + TWAP — aero_cl_head.rs, aero_first_fee.rs) плюс налог.
//! Кто вызывает: pools/load.rs (ветка Slipstream, вариант SlipstreamAero), exec/solidly_gate.rs (fresh_cap),
//! pools/per_block_aero.rs (hook_tax).
//! От чего зависит: ctx.rs, node/{call, rpc}.rs, pools/{aero_mev, aero_cl_head}.rs.
//! Настройки: dex.factory, dex.cl_swap_hook, dex.cl_hook_ahead_s [DEX]; через aero_mev.rs — dex.gas_mode, send.*
//! [площадка DEX].
//!
//! Источник — Dromos-Labs/metadex-slipstream-public 1.1.0 (cf1ae46f) и metadex-public 1.1.0 (79c8d5b6):
//! - CLPool.swap L497-L721: комиссия цикла state.fee = крючок beforeSwap (L543-L556; SwapHookLib.beforeSwap L106-L118:
//!   ненулевая — min(fee, 100000), иначе tickSpacingToFee); после цикла afterSwap может добавить комиссию к выходу
//!   (SwapHookLib L126-L170) — у DynamicSwapFeeHook afterSwap её не возвращает (DynamicSwapFeeHook.sol L310-L389, нет
//!   return — 0; getAfterSwapFee L420-L425 = 0). Обмен — swap(recipient, zeroForOne, amountSpecified,
//!   sqrtPriceLimitX96, data) + uniswapV3SwapCallback (L712, L718) — как у Uniswap V3: шаг V3 исполнителя.
//! - fee() пула (CLPool.sol L188-L194) — beforeSwapFee с caller = msg.sender (eth_call без from — нулевой адрес, без
//!   скидки DiscountRegistry: скидка только уменьшает комиссию, DynamicSwapFeeHook.sol L673-L697) =
//!   крючок getBeforeSwapFee (L406-L417): комиссия блока blockFee[pool][номер блока Arbitrum] или, если в блоке обменов
//!   не было, — первая комиссия блока (база + |тик − TWAP-тик|·масштаб/1e6, не выше предела, L569-L667) + MEV-налог.
//!   Решение 470 (T470-01): комиссия того блока, где окажется наша транзакция, — первая комиссия нового блока (TWAP по
//!   его времени) или blockFee чужого первого обмена (та же формула, feeToStore): котировка берёт наибольшую из fee()
//!   головы и первой комиссии на время головы + δ, δ = 0…dex.cl_hook_ahead_s (aero_first_fee.rs worst). Каждым блоком —
//!   лёгкое чтение пачкой (per_block_aero.rs), полное — по событиям пула; перед отправкой — заново (fee_latest).

use alloy_primitives::{Address, U64};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::{call, rpc};
use super::families::Family;
use super::{aero_cl_head, aero_mev};

alloy::sol! {
    interface IAeroClFactory {
        function getPoolSwapHook(address pool) external view returns (address);
    }

    interface IAeroSwapHook {
        function mevTaxModule() external view returns (address);
    }
}

/// Проверки пула и его комиссия на блоке. Принимает: окружение, вид (id DEX), пул, tickSpacing пула, номер блока.
/// Возвращает: комиссию (pips, над 1e6) — наибольшая из fee() пула и первой комиссии следующего блока (T470-01,
/// aero_cl_head.rs) плюс MEV-налог. Ошибки: не заданы dex.factory / dex.cl_swap_hook / dex.cl_hook_ahead_s;
/// NotApplicable — крючок пула не тот, MEV-налог (aero_mev.rs); узел, ответ не по ABI.
pub async fn fee(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    spacing: i32,
    block: u64,
) -> GatewayResult<u32> {
    let tag = call::block_tag(block);
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let want = ctx.dex_addr("dex.cl_swap_hook", f.id)?;
    let ask = IAeroClFactory::getPoolSwapHookCall { pool };
    let hook = call::view(ctx, factory, &ask, &tag).await?;
    if hook != want {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            format!("Aero Slipstream V3 {pool}: крючок пула {hook} ≠ dex.cl_swap_hook {want}"),
        ));
    }
    let tax = hook_tax(ctx, f, &tag).await?;
    let read = aero_cl_head::read(ctx, f, &[(pool, spacing)], block).await?;
    match read.into_iter().next() {
        Some(Ok(view)) => Ok(total(view, tax)),
        Some(Err(why)) => Err(ctx.err(
            ErrorCode::BadResponse,
            format!("Aero Slipstream V3 {pool}: {why}"),
        )),
        None => Err(ctx.err(ErrorCode::Internal, "Aero Slipstream V3: нет ответа пачки")),
    }
}

/// MEV-налог крючка dex.cl_swap_hook (модуль крючка, aero_mev.rs) на блоке. Принимает: окружение, вид, тег блока.
/// Ошибки: не задана dex.cl_swap_hook; как aero_mev::tax.
pub async fn hook_tax(ctx: &Ctx, f: &Family, tag: &str) -> GatewayResult<u32> {
    let hook = ctx.dex_addr("dex.cl_swap_hook", f.id)?;
    let module = call::view(ctx, hook, &IAeroSwapHook::mevTaxModuleCall {}, tag).await?;
    aero_mev::tax(ctx, module, tag).await
}

/// То же, что fee, на последнем блоке узла (перед отправкой, exec/solidly_gate.rs). Ошибки: как fee; узел
/// (eth_blockNumber).
pub async fn fee_latest(ctx: &Ctx, f: &Family, pool: Address, spacing: i32) -> GatewayResult<u32> {
    let n: U64 = rpc::call(ctx, "eth_blockNumber", serde_json::json!([])).await?;
    fee(ctx, f, pool, spacing, n.to::<u64>()).await
}

/// Комиссия цикла: fee() пула (eth_call — налог 0) + налог, не выше 100000 (SwapHookLib.cap L54-L59, _SWAP_FEE_CEIL
/// L24; крючок складывает «динамическая + налог» — DynamicSwapFeeHook.sol L296-L306). Ошибок нет.
pub fn total(view: u32, tax: u32) -> u32 {
    view.saturating_add(tax).min(100_000)
}
