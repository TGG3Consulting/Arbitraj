//! Файл: venues/bsc_dex/pools/aero_cl.rs
//! Что делает: решение 470 (T451-01) — комиссия и проверки пула Aero «Slipstream V3» (вид slipstream_aero: тот же
//! CLPool, что у вида slipstream, но комиссия обмена — от крючка). На блоке: крючок пула у фабрики
//! (getPoolSwapHook) равен заданному dex.cl_swap_hook (иначе пул не читается: поведение чужого крючка не сверено);
//! правило MEV-налога модуля крючка (aero_mev.rs); комиссия — fee() пула на этом блоке.
//! Кто вызывает: pools/load.rs (ветка Slipstream, вариант SlipstreamAero), exec/solidly_gate.rs (fresh_cap).
//! От чего зависит: ctx.rs, node/call.rs, pools/{aero_mev, fee_fresh}.rs.
//! Настройки: dex.factory, dex.cl_swap_hook [DEX]; через aero_mev.rs — dex.gas_mode, send.* [площадка DEX].
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
//!   ОГОВОРКА: комиссия того блока, где окажется наша транзакция, может отличаться (TWAP по времени нового блока,
//!   чужие обмены раньше в блоке) — читается каждым блоком (per_block.rs), перед отправкой — заново (fresh).

use alloy_primitives::Address;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::aero_mev;
use super::families::Family;
use super::fee_fresh;

alloy::sol! {
    interface IAeroClFactory {
        function getPoolSwapHook(address pool) external view returns (address);
    }

    interface IAeroSwapHook {
        function mevTaxModule() external view returns (address);
    }
}

/// Проверки пула и его комиссия на блоке. Принимает: окружение, вид (id DEX), пул, тег. Возвращает: комиссию (pips,
/// над 1e6). Ошибки: не заданы dex.factory / dex.cl_swap_hook; NotApplicable — крючок пула не тот, MEV-налог возможен
/// (aero_mev.rs); узел, ответ не по ABI.
pub async fn fee(ctx: &Ctx, f: &Family, pool: Address, tag: &str) -> GatewayResult<u32> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let want = ctx.dex_addr("dex.cl_swap_hook", f.id)?;
    let ask = IAeroClFactory::getPoolSwapHookCall { pool };
    let hook = call::view(ctx, factory, &ask, tag).await?;
    if hook != want {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            format!("Aero Slipstream V3 {pool}: крючок пула {hook} ≠ dex.cl_swap_hook {want}"),
        ));
    }
    let module = call::view(ctx, hook, &IAeroSwapHook::mevTaxModuleCall {}, tag).await?;
    let tax = aero_mev::tax(ctx, module, tag).await?;
    let view = fee_fresh::read(ctx, pool, tag).await?;
    Ok(total(view, tax))
}

/// Комиссия цикла: fee() пула (eth_call — налог 0) + налог, не выше 100000 (SwapHookLib.cap L54-L59, _SWAP_FEE_CEIL
/// L24; крючок складывает «динамическая + налог» — DynamicSwapFeeHook.sol L296-L306). Ошибок нет.
pub fn total(view: u32, tax: u32) -> u32 {
    view.saturating_add(tax).min(100_000)
}
