//! Файл: venues/bsc_dex/exec/plan.rs
//! Что делает: план прямого обмена в одном пуле через Universal Router своей DEX: недостающие разрешения (Permit2)
//! и вызов execute — обмен + BALANCE_CHECK_ERC20 «баланс выходного токена кошелька ≥ текущий + минимум»
//! (решение 40: проверка реально полученного — всегда, в той же транзакции).
//! Кто вызывает: exec/trade.rs, check/probe.rs.
//! От чего зависит: exec/ur.rs, exec/swap_v2v3.rs, exec/swap_v4.rs, wallet/approve.rs, wallet/balances.rs, pools/registry.rs.
//! Настройки: dex.universal_router, dex.permit2, dex.universal_router_version [вид пула], dex.deadline_s,
//! dex.approve_unlimited, dex.permit2_expiration_s [bsc_dex].
//!
//! Почему Universal Router для всех видов: только в нём есть проверка баланса получателя в той же транзакции
//! (BALANCE_CHECK_ERC20 — Commands.sol #L31 у обоих UR), и одна схема разрешений (Permit2) для V2, V3 и V4.

use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolKind;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::super::wallet::approve::{self, Approval};
use super::super::wallet::balances;
use super::{swap_v2v3, swap_v4, ur};

/// План обмена.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Разрешения, которые надо отправить до обмена (по порядку).
    pub approvals: Vec<Approval>,
    /// Universal Router и данные execute.
    pub router: Address,
    pub data: Bytes,
}

/// Что меняем.
pub struct Order {
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    /// Минимум реально полученного выходного токена.
    pub min_out: U256,
}

/// Строит план для кошелька. Принимает: пул, кошелёк, обмен, время блока (секунды), нужны ли все разрешения
/// (true — для симуляции, без чтения текущих). Ошибки: NotConfigured (адреса, версия, срок), узел.
pub async fn build(
    ctx: &Ctx,
    e: &Entry,
    wallet: Address,
    o: &Order,
    now: u64,
    all_approvals: bool,
) -> GatewayResult<Plan> {
    let router = ctx.dex_addr("dex.universal_router", e.family.id)?;
    let permit2 = ctx.dex_addr("dex.permit2", e.family.id)?;
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    let fmt = ur::format(ctx, e)?;
    let approvals = if all_approvals {
        approve::all(ctx, o.token_in, permit2, router, o.amount_in, now)?
    } else {
        approve::missing(ctx, o.token_in, wallet, permit2, router, o.amount_in, now).await?
    };
    let swap = match e.pool.kind {
        PoolKind::V2 | PoolKind::V3 => swap_v2v3::command(ctx, e, fmt, o, wallet)?,
        PoolKind::Cl | PoolKind::Bin => swap_v4::command(ctx, e, fmt, o)?,
    };
    let before = balances::token_units(ctx, o.token_out, wallet).await?;
    let check = (
        ur::BALANCE_CHECK_ERC20,
        ur::balance_check(wallet, o.token_out, before.saturating_add(o.min_out)),
    );
    Ok(Plan {
        approvals,
        router,
        data: ur::execute(vec![swap, check], deadline),
    })
}
