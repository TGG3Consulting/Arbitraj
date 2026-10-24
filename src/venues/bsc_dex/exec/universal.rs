//! Файл: venues/bsc_dex/exec/universal.rs
//! Что делает: план умного маршрута для Universal Router PancakeSwap Infinity (решение 38): у каждой доли свои команды —
//! один пул или два через промежуточную монету (решение 52, exec/chain.rs: первый шаг платит кошелёк через Permit2,
//! второй — роутер своим балансом), минимум выхода — у каждого шага; последней командой — BALANCE_CHECK_ERC20
//! «баланс выходного токена кошелька ≥ база + сумма минимумов долей» (решение 40; база читается прямо перед подписью —
//! exec/plan.rs, Plan::data). Разрешения: ERC-20 → Permit2 и Permit2 → Universal Router на сумму входов всех долей.
//! Кто вызывает: exec/trade_smart.rs.
//! От чего зависит: exec/chain.rs, exec/plan.rs (Plan), exec/ur.rs (Format), wallet/approve.rs, pools/families.rs.
//! Настройки: smart.universal_router, smart.permit2, dex.deadline_s [bsc_dex]; через вызываемые —
//! dex.approve_unlimited, dex.permit2_expiration_s.
//!
//! Правила официального SDK (research/flow/bsc_smart_router.md, раздел 3.1, URSDK dist/index.mjs#L280-L590):
//! split — независимые цепочки команд в одном execute, у каждой своя доля входа и свой minimumAmountOut;
//! payerIsUser = true у первой секции цепочки; промежуточный получатель — ADDRESS_THIS, вход следующей секции —
//! CONTRACT_BALANCE. Соседние секции здесь не сливаются (каждый пул — своя команда): у каждой свой минимум.

use alloy_primitives::{Address, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::families::smart_ok;
use super::super::wallet::approve;
use super::chain::{self, Step};
use super::plan::Plan;
use super::ur::Format;

/// План умного маршрута. Принимает: доли — у каждой шаги (exec/chain.rs: один пул или два через промежуточную), у всех
/// один токен входа и один выхода; кошелёк; время блока (секунды). Ошибки: NotConfigured (адреса, срок), Internal
/// (пустой маршрут, разные токены, пул не PancakeSwap, минимум шага 0), узел (разрешения).
pub async fn build(
    ctx: &Ctx,
    legs: &[Vec<Step>],
    wallet: Address,
    now: u64,
) -> GatewayResult<Plan> {
    let ends = |l: &Vec<Step>| match (l.first(), l.last()) {
        (Some(a), Some(b)) => Some((a.order.token_in, b.order.token_out)),
        _ => None,
    };
    let (tin, tout) = legs
        .first()
        .and_then(ends)
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "умный маршрут без долей"))?;
    let router = ctx.addr("smart.universal_router")?;
    let permit2 = ctx.addr("smart.permit2")?;
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    let mut commands = Vec::new();
    let (mut total_in, mut total_min) = (U256::ZERO, U256::ZERO);
    for leg in legs {
        if ends(leg) != Some((tin, tout)) {
            return Err(ctx.err(ErrorCode::Internal, "доля не той пары токенов"));
        }
        for (i, s) in leg.iter().enumerate() {
            if !smart_ok(ctx, s.entry.family) || s.order.min_out.is_zero() {
                return Err(ctx.err(
                    ErrorCode::Internal,
                    format!("{}: шаг не исполняет умный роутер (dex.universal_router ≠ smart.universal_router) или минимум 0", s.entry.family.title),
                ));
            }
            // Платит кошелёк — сумма в разрешения; нативная продажа (решение 62) — платит роутер из WRAP_ETH.
            if i == 0 && s.from_wallet {
                total_in = total_in.saturating_add(s.order.amount_in);
            }
            commands.push(chain::command(ctx, s, Format::Pancake, wallet)?);
        }
        if let Some(last) = leg.last() {
            total_min = total_min.saturating_add(last.order.min_out);
        }
    }
    let approvals = if total_in.is_zero() {
        Vec::new()
    } else {
        approve::missing(ctx, tin, wallet, permit2, router, total_in, now).await?
    };
    Plan::new(
        ctx,
        (approvals, router, wallet),
        tout,
        commands,
        total_min,
        deadline,
    )
}
