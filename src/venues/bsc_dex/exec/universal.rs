//! Файл: venues/bsc_dex/exec/universal.rs
//! Что делает: план умного маршрута для Universal Router PancakeSwap Infinity (решение 38): каждая доля — своя
//! команда обмена в своём пуле (V2_SWAP_EXACT_IN / V3_SWAP_EXACT_IN / INFI_SWAP), платит кошелёк через Permit2
//! (payerIsUser = true), получатель — кошелёк, минимум выхода — свой у каждой доли; последней командой —
//! BALANCE_CHECK_ERC20 «баланс выходного токена кошелька ≥ база + сумма минимумов» (решение 40: проверка реально
//! полученного в той же транзакции; база читается прямо перед подписью — exec/plan.rs, Plan::data). Разрешения: ERC-20 → Permit2 и Permit2 → Universal Router на сумму всех долей.
//! Кто вызывает: exec/trade_smart.rs.
//! От чего зависит: exec/ur.rs, exec/swap_v2v3.rs, exec/swap_v4.rs, exec/plan.rs (Plan, Order), wallet/approve.rs,
//! pools/registry.rs.
//! Настройки: smart.universal_router, smart.permit2, dex.deadline_s [bsc_dex]; через вызываемые —
//! dex.approve_unlimited, dex.permit2_expiration_s.
//!
//! Правила официального SDK (research/flow/bsc_smart_router.md, раздел 3.1, URSDK dist/index.mjs#L280-L590):
//! split — независимые цепочки команд в одном execute, у каждой своя доля входа и свой minimumAmountOut;
//! payerIsUser = true у первой секции цепочки. Доля здесь — один пул (одна секция), поэтому промежуточных
//! получателей (ADDRESS_THIS, адрес следующей пары V2) и слияния соседних секций Infinity не требуется.
//! Входы команд — exec/swap_v2v3.rs и exec/swap_v4.rs в формате PancakeSwap (PUR-D/src/base/Dispatcher.sol).

use alloy_primitives::{Address, U256};

use crate::core::types::PoolKind;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::families::PANCAKESWAP;
use super::super::pools::registry::Entry;
use super::super::wallet::approve;
use super::plan::{Order, Plan};
use super::ur::Format;
use super::{swap_v2v3, swap_v4};

/// План умного маршрута. Принимает: доли (пул, обмен) — у всех один токен входа и один выхода; кошелёк;
/// время блока (секунды). Ошибки: NotConfigured (адреса, срок), Internal (пустой маршрут, разные токены,
/// пул не PancakeSwap, минимум доли 0), узел (разрешения).
pub async fn build(
    ctx: &Ctx,
    legs: &[(Entry, Order)],
    wallet: Address,
    now: u64,
) -> GatewayResult<Plan> {
    let (_, first) = legs
        .first()
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "умный маршрут без долей"))?;
    let (tin, tout) = (first.token_in, first.token_out);
    let router = ctx.addr("smart.universal_router")?;
    let permit2 = ctx.addr("smart.permit2")?;
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    let mut steps = Vec::with_capacity(legs.len() + 1);
    let (mut total_in, mut total_min) = (U256::ZERO, U256::ZERO);
    for (e, o) in legs {
        if o.token_in != tin || o.token_out != tout || e.family.dex != PANCAKESWAP {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!(
                    "{}: доля не той пары токенов или не PancakeSwap",
                    e.family.title
                ),
            ));
        }
        if o.min_out.is_zero() {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!(
                    "{}: минимум доли 0 — обмен без минимума не отправляется",
                    e.family.title
                ),
            ));
        }
        total_in = total_in.saturating_add(o.amount_in);
        total_min = total_min.saturating_add(o.min_out);
        steps.push(match e.pool.kind {
            PoolKind::V2 | PoolKind::V3 => swap_v2v3::command(ctx, e, Format::Pancake, o, wallet)?,
            PoolKind::Cl | PoolKind::Bin => swap_v4::command(ctx, e, Format::Pancake, o)?,
        });
    }
    let approvals = approve::missing(ctx, tin, wallet, permit2, router, total_in, now).await?;
    Plan::new(
        ctx,
        (approvals, router, wallet),
        tout,
        steps,
        total_min,
        deadline,
    )
}
