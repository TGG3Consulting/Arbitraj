//! Файл: venues/bsc_dex/pools/load_aero_basic.rs
//! Что делает: решение 470 (T451-02) — состояние пула Aero «Basic» (VolatilePool / StablePool, Arbitrum) на блоке в
//! PoolState::Solidly: резервы и 10^decimals — metadata() пула (6 полей, без stable), кривая — POOL_TYPE() пула,
//! комиссия — getBaseFee фабрики (числитель над 10000), пауза — isPaused() фабрики, MEV-налог — правило aero_mev.rs.
//! Математика обмена та же, что у Solidly (math/v2.rs, math/swap_solidly.rs): сверено построчно — см. ниже.
//! Кто вызывает: pools/load.rs (ветка Proto::AeroBasic), pools/solidly_refresh.rs.
//! От чего зависит: ctx.rs, node/call.rs, pools/{abi_aero_basic, aero_mev, load_solidly}.rs, math/solidly_math.rs.
//! Настройки: dex.factory [DEX]; через aero_mev.rs — dex.gas_mode, send.tip_mode, send.priority_fee_gwei,
//! send.tip_cap_gwei [площадка DEX].
//!
//! Источник — Dromos-Labs/metadex-public 1.1.0 (79c8d5b6), V3/src:
//! - комиссия обмена: Pool.swap L172 → factory.getFee L282-L306 = getBaseFee (модуль комиссии, при сбое/больше
//!   MAX_BASE_FEE — defaultFee, L348-L371) + MEV-налог (L291-L305). Модули Arbitrum (pools-arbitrum.json,
//!   volatile/stableCustomFeeModule) — CustomFeeModule.getFee (fees/CustomFeeModule.sol L34-L39): зависит ТОЛЬКО от
//!   адреса пула (caller, суммы и резервы не читает) — поэтому getBaseFee читается с caller = 0 и суммами 0, как его
//!   же зовёт Pool.getAmountOut L537-L545 (caller = 0). Применение — `amountIn − amountIn·fee/10000` (L174, L180,
//!   L541-L543; _MAX_BPS = 10000 L35) — правило «числитель над 10000» вида Solidly;
//! - кривые: VolatilePool._getAmountOut L17-L25 = x·y=k; StablePool._getAmountOut L73-L86, _k L108-L114, _getY L150-L183
//!   = Velodrome Pool._getAmountOut/_k/_get_y (math/swap_solidly.rs). Отличие одно: Aero умножает через Math.mulDiv
//!   (полная точность), Velodrome — `a·b/1e18` checked: при равных результатах чисел без переполнения 2^256 совпадают; где
//!   у Velodrome было бы переполнение, наш порт отдаёт None (котировки нет), Aero посчитал бы — отказ, а не неверное число.

use alloy_primitives::{Address, U256};

use crate::core::types::{FeeRule, PoolState, SolidlyFee, SolidlyGate};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::solidly_math::fee_denominator;
use super::super::node::call;
use super::abi_aero_basic::{
    IAeroBasicFactory as F, IAeroBasicPool as P, stable_type, volatile_type,
};
use super::aero_mev;
use super::families::Family;
use super::load_solidly::fee_u32;

/// Состояние пула Aero Basic на блоке. Принимает: окружение, вид (id DEX — для dex.factory), адрес пула, тег блока.
/// Ошибки: не задана dex.factory; узел; ответ не по ABI; POOL_TYPE не 'V2_STABLE'/'V2_VOLATILE'; комиссия ≥ 100%;
/// MEV-налог возможен (aero_mev.rs).
pub async fn load(ctx: &Ctx, f: &Family, pool: Address, tag: &str) -> GatewayResult<PoolState> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let meta = call::view(ctx, pool, &P::metadataCall {}, tag).await?;
    let kind = call::view(ctx, pool, &P::POOL_TYPECall {}, tag).await?;
    let stable = if kind == stable_type() {
        true
    } else if kind == volatile_type() {
        false
    } else {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!("Aero Basic {pool}: POOL_TYPE {kind} — не V2_STABLE / V2_VOLATILE"),
        ));
    };
    let ask = F::getBaseFeeCall {
        pool,
        caller: Address::ZERO,
        amount0In: U256::ZERO,
        amount1In: U256::ZERO,
        reserve0: meta.r0,
        reserve1: meta.r1,
    };
    let fee = call::view(ctx, factory, &ask, tag).await?;
    let paused = call::view(ctx, factory, &F::isPausedCall {}, tag).await?;
    let module = call::view(ctx, factory, &F::mevTaxModuleCall {}, tag).await?;
    aero_mev::check(ctx, module, tag).await?;
    Ok(PoolState::Solidly {
        reserve0: meta.r0,
        reserve1: meta.r1,
        stable,
        decimals0: meta.dec0,
        decimals1: meta.dec1,
        fee: SolidlyFee {
            value: fee_u32(ctx, fee)?,
            rule: FeeRule::Numerator {
                denominator: fee_denominator().to::<u32>(),
            },
        },
        gate: SolidlyGate {
            paused,
            max_gas_price: None,
        },
        fee1: None,
    })
}
