//! Файл: venues/bsc_dex/exec/plan.rs
//! Что делает: план обмена через Universal Router своей DEX (один пул или путь через промежуточную монету — по команде
//! на шаг, exec/chain.rs): недостающие разрешения (Permit2)
//! и вызов execute — обмен + BALANCE_CHECK_ERC20 «баланс выходного токена кошелька ≥ база + минимум»
//! (решение 40: проверка реально полученного — всегда, в той же транзакции). База — баланс кошелька, прочитанный
//! прямо перед подписью обмена (после разрешений): данные execute собираются только тогда (`Plan::data`).
//! Кто вызывает: exec/trade.rs, check/probe.rs.
//! От чего зависит: exec/ur.rs, exec/chain.rs, wallet/approve.rs, pools/registry.rs.
//! Настройки: dex.universal_router, dex.permit2, dex.universal_router_version [вид пула], dex.deadline_s,
//! dex.approve_unlimited, dex.permit2_expiration_s [bsc_dex].
//!
//! Почему Universal Router для всех видов: только в нём есть проверка баланса получателя в той же транзакции
//! (BALANCE_CHECK_ERC20 — Commands.sol #L31 у обоих UR), и одна схема разрешений (Permit2) для V2, V3 и V4.
//! «never send amountMin = 0» — руководство PancakeSwap для агентов (research/flow/bsc_dex.md, раздел 5):
//! минимум 0 — ошибка, план не строится.

use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::super::wallet::approve::{self, Approval};
use super::chain::{self, Step};
use super::ur;

/// План обмена.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Разрешения, которые надо отправить до обмена (по порядку).
    pub approvals: Vec<Approval>,
    /// Universal Router.
    pub router: Address,
    /// Кошелёк и выходной токен для проверки баланса.
    pub wallet: Address,
    pub token_out: Address,
    /// Команды обмена (без проверки баланса).
    steps: Vec<(u8, Bytes)>,
    /// Сумма минимумов выхода (не 0).
    min_total: U256,
    deadline: U256,
}

impl Plan {
    /// Собирает план. Принимает: разрешения, роутер, кошелёк, выходной токен, команды, сумму минимумов, срок.
    /// Ошибки: Internal — сумма минимумов 0 (обмен без минимума не отправляется).
    pub fn new(
        ctx: &Ctx,
        (approvals, router, wallet): (Vec<Approval>, Address, Address),
        token_out: Address,
        steps: Vec<(u8, Bytes)>,
        min_total: U256,
        deadline: U256,
    ) -> GatewayResult<Self> {
        if min_total.is_zero() {
            return Err(ctx.err(
                ErrorCode::Internal,
                "минимум выхода 0 — обмен без минимума не отправляется",
            ));
        }
        Ok(Self {
            approvals,
            router,
            wallet,
            token_out,
            steps,
            min_total,
            deadline,
        })
    }

    /// Данные execute: команды обмена + BALANCE_CHECK_ERC20(кошелёк, токен, база + сумма минимумов).
    /// Принимает: базу — баланс выходного токена кошелька прямо перед подписью. Ошибок нет.
    pub fn data(&self, base: U256) -> Bytes {
        let mut steps = self.steps.clone();
        steps.push((
            ur::BALANCE_CHECK_ERC20,
            ur::balance_check(
                self.wallet,
                self.token_out,
                base.saturating_add(self.min_total),
            ),
        ));
        ur::execute(steps, self.deadline)
    }
}

/// Что меняем.
pub struct Order {
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    /// Минимум реально полученного выходного токена (не 0).
    pub min_out: U256,
}

/// Строит план для кошелька через Universal Router своей DEX (вид пула первого шага; у пути через промежуточную
/// монету оба пула — одного Universal Router, quote/direct_route.rs). Принимает: шаги (exec/chain.rs), кошелёк, время
/// блока (секунды), нужны ли все разрешения (true — для симуляции, без чтения текущих).
/// Ошибки: NotConfigured (адреса, версия, срок), Internal (нет шагов, минимум 0), узел.
pub async fn build(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all_approvals: bool,
) -> GatewayResult<Plan> {
    let (Some(first), Some(last)) = (steps.first(), steps.last()) else {
        return Err(ctx.err(ErrorCode::Internal, "план без шагов"));
    };
    let e = &first.entry;
    let router = ctx.dex_addr("dex.universal_router", e.family.id)?;
    let permit2 = ctx.dex_addr("dex.permit2", e.family.id)?;
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    let fmt = ur::format(ctx, e)?;
    let o = &first.order;
    let approvals = if all_approvals {
        approve::all(ctx, o.token_in, permit2, router, o.amount_in, now)?
    } else {
        approve::missing(ctx, o.token_in, wallet, permit2, router, o.amount_in, now).await?
    };
    let mut commands = Vec::with_capacity(steps.len());
    for s in steps {
        if s.order.min_out.is_zero() {
            return Err(ctx.err(
                ErrorCode::Internal,
                "минимум шага 0 — обмен без минимума не отправляется",
            ));
        }
        commands.push(chain::command(ctx, s, fmt, wallet)?);
    }
    Plan::new(
        ctx,
        (approvals, router, wallet),
        last.order.token_out,
        commands,
        last.order.min_out,
        deadline,
    )
}

/// Один шаг «кошелёк → пул → кошелёк» (пробный обмен, прямой обмен в одном пуле). Ошибок нет.
pub fn single(e: &Entry, order: Order) -> Vec<Step> {
    vec![Step {
        entry: e.clone(),
        order,
        from_wallet: true,
        to_wallet: true,
    }]
}
