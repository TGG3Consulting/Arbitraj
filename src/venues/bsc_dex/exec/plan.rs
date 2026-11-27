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
//! Нативная монета (решение 62, exec/native.rs): продажа — первой командой WRAP_ETH (BNB транзакции → WBNB роутера),
//! разрешений нет (платит роутер); покупка — последней командой UNWRAP_WETH(кошелёк, сумма минимумов) вместо
//! BALANCE_CHECK_ERC20: роутер разворачивает весь полученный WBNB и шлёт BNB кошельку, меньше минимума — откат всей
//! транзакции (та же проверка реально полученного, решение 40, — по балансу роутера, а не кошелька).

use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::super::wallet::approve::{self, Approval};
use super::chain::{self, Step};
use super::native::Wrap;
use super::ur;

#[derive(Debug, Clone)]
pub(super) enum DirectCall {
    RegisteredV3(std::sync::Arc<super::registered_v3_wallet::Proof>),
    Carbon(std::sync::Arc<super::carbon_wire::Call>),
    Temple(std::sync::Arc<super::temple_wallet::Proof>),
    Antfarm(std::sync::Arc<super::antfarm_wallet::Proof>),
    AntfarmOutput(std::sync::Arc<super::antfarm_output_wallet::Proof>),
    MintClub(super::mint_club_wire::Call),
    EkuboV2 {key:super::super::pools::ekubo_v2::OrderKey,core:Address,wrapped:Option<Address>},
    Module {
        pool: Address,
        input: Address,
        amount: U256,
    },
    Vista {
        call: super::vista_wire::Call,
        input: Address,
        amount: U256,
    },
    Oasis {
        input: Address,
        amount: U256,
        rebate: bool,
    },
    Vault { id: alloy_primitives::B256, input: Address, amount: U256 },
    Umbrae {path:super::umbrae::Path,input:Address,amount:U256},
    SectorLb { path:super::sectorone::Path,input:Address,amount:U256 },
    V2SupportingFee {
        input: Address,
        amount: U256,
    },
}

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
    /// Аргументы прямого модуля; он сам измеряет выход на кошельке в том же вызове.
    direct_market: Option<Box<DirectCall>>,
    /// Сумма минимумов выхода (не 0).
    min_total: U256,
    deadline: U256,
    /// Нативная монета (решение 62): без неё / обернуть BNB транзакции / развернуть выход на кошелёк.
    native: Wrap,
}

impl Plan {
    pub(super) fn direct_market(&self) -> Option<DirectCall> {
        self.direct_market.as_deref().cloned()
    }
    pub(super) fn set_direct_market(&mut self, call: DirectCall) {
        self.direct_market = Some(Box::new(call));
    }

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
            direct_market: None,
            min_total,
            deadline,
            native: Wrap::None,
        })
    }

    /// План с нативной монетой (exec/native.rs::adapt). Ошибок нет.
    pub fn with_native(mut self, w: Wrap) -> Self {
        self.native = w;
        self
    }

    /// Решение 248: план с другой суммой минимумов (проверка баланса BALANCE_CHECK_ERC20 — база + она) — пере-замер
    /// Universal Router (exec/ur_remeasure.rs): минимумы шагов 1 ед., защита выхода — проверка баланса. Принимает:
    /// сумму (не 0). Ошибки: Internal — сумма 0 (обмен без минимума не отправляется).
    pub fn with_min_total(mut self, ctx: &Ctx, m: U256) -> GatewayResult<Self> {
        if m.is_zero() {
            return Err(ctx.err(
                ErrorCode::Internal,
                "минимум выхода 0 — обмен без минимума не отправляется",
            ));
        }
        if let Some(DirectCall::RegisteredV3(proof)) = self.direct_market.as_deref_mut() {
            std::sync::Arc::make_mut(proof).minimum = m;
        }
        self.min_total = m;
        Ok(self)
    }

    /// Сумма минимумов выхода (единицы токена выхода). Ошибок нет.
    pub fn min_total(&self) -> U256 {
        self.min_total
    }

    /// Сколько BNB (wei) уходит с транзакцией обмена: сумма WRAP_ETH или 0. Ошибок нет.
    pub(super) fn ekubo_proof(&self)->Option<super::ekubo_v2_fact::Proof>{super::ekubo_v2_fact::Proof::of(self,self.deadline)}
    pub fn value(&self) -> U256 {
        if let Some(DirectCall::Carbon(c))=self.direct_market(){return c.value();}
        if let Some(DirectCall::EkuboV2{key,..})=self.direct_market(){return key.value();}
        if let Some(DirectCall::Vista { call, amount, .. }) = self.direct_market() {
            return call.value(amount);
        }
        match self.native {
            Wrap::Wrap(v) => v,
            _ => U256::ZERO,
        }
    }

    /// Данные execute: команды обмена + BALANCE_CHECK_ERC20(кошелёк, токен, база + сумма минимумов).
    /// Принимает: базу — баланс выходного токена кошелька прямо перед подписью. Ошибок нет.
    /// Нативная монета: WRAP_ETH(роутер, сумма) первой командой; покупка — UNWRAP_WETH(кошелёк, сумма минимумов)
    /// последней вместо BALANCE_CHECK_ERC20 (база тогда не нужна).
    pub fn data(&self, base: U256) -> Bytes {
        if let Some(direct) = self.direct_market() {
            return match direct {
                DirectCall::Module {
                    pool,
                    input,
                    amount,
                } => super::super::pools::solidly_v3::execution_data(
                    pool,
                    input,
                    self.token_out,
                    amount,
                    self.min_total,
                    self.deadline,
                ),
                DirectCall::Vista {
                    call,
                    input,
                    amount,
                } => call.data(
                    input,
                    self.token_out,
                    amount,
                    self.min_total,
                    self.wallet,
                    self.deadline,
                ),
                DirectCall::Oasis {
                    input,
                    amount,
                    rebate,
                } => super::oasis_direct::swap_data(
                    input,
                    self.token_out,
                    amount,
                    self.min_total,
                    self.wallet,
                    self.deadline,
                    rebate,
                ),
                DirectCall::Carbon(c)=>c.data(),
                DirectCall::Temple(t)=>t.data(),
                DirectCall::Antfarm(a)=>a.data(),
                DirectCall::AntfarmOutput(a)=>a.data(),
                DirectCall::MintClub(order)=>order.data(self.wallet),
                DirectCall::EkuboV2{key,..}=>key.calldata(self.min_total,self.deadline),
                DirectCall::Vault { id, input, amount } => super::vault_direct::data(
                    id, input, self.token_out, amount, self.min_total, self.wallet, self.deadline),
                DirectCall::RegisteredV3(proof) => proof.data(),
                DirectCall::Umbrae{path,input,amount}=>path.data(input,self.token_out,amount,self.min_total,self.wallet,self.deadline),
                DirectCall::SectorLb{path,input,amount}=>path.data(input,self.token_out,amount,self.min_total,self.wallet,self.deadline),
                DirectCall::V2SupportingFee { input, amount } => super::fork_swap::swap_data(
                    amount,
                    self.min_total,
                    &[input, self.token_out],
                    self.wallet,
                    self.deadline,
                ),
            };
        }
        let mut steps = Vec::with_capacity(self.steps.len() + 2);
        if let Wrap::Wrap(v) = self.native {
            steps.push((ur::WRAP_ETH, ur::native(ur::ADDRESS_THIS, v)));
        }
        steps.extend(self.steps.iter().cloned());
        let last = match self.native {
            Wrap::Unwrap => (ur::UNWRAP_WETH, ur::native(self.wallet, self.min_total)),
            _ => (
                ur::BALANCE_CHECK_ERC20,
                ur::balance_check(
                    self.wallet,
                    self.token_out,
                    base.saturating_add(self.min_total),
                ),
            ),
        };
        steps.push(last);
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
    if let Some(market)=super::market_plan::Market::of(e){
        return Err(ctx.err(ErrorCode::NoRoute,market.missing_selection()));
    }
    if super::super::quote::ekubo_v2_pricing::enabled(e) {return Box::pin(super::ekubo_v2_direct::plan(ctx,steps,wallet,now,all_approvals)).await;}
    if super::vault_direct::enabled(e) {
        return Box::pin(super::vault_direct::plan(ctx,steps,wallet,now,all_approvals)).await;
    }
    if super::super::quote::vista_pricing::is_vista(e) || (super::vista_direct::native() && super::super::quote::vista_pricing::native_router(e)) {
        return Box::pin(super::vista_direct::plan(
            ctx,
            steps,
            wallet,
            now,
            all_approvals,
        ))
        .await;
    }
    if super::super::quote::oasis_pricing::is_oasis(e) {
        return Box::pin(super::oasis_direct::plan(
            ctx,
            steps,
            wallet,
            now,
            all_approvals,
        ))
        .await;
    }
    if super::super::pools::sectorone::enabled(e) {return Box::pin(super::sectorone::plan(ctx,steps,wallet,now,all_approvals)).await;}
    if super::super::quote::frax_pricing::is_frax(e) {
        return Box::pin(super::frax_direct::plan(
            ctx,
            steps,
            wallet,
            now,
            all_approvals,
        ))
        .await;
    }
    if e.family.variant == super::super::pools::family_variant::Variant::DeltaSwap {
        return Box::pin(super::delta_direct::plan(
            ctx,
            steps,
            wallet,
            now,
            all_approvals,
        ))
        .await;
    }
    if e.family.variant == super::super::pools::family_variant::Variant::WhiteSwap
        || super::super::quote::router_pricing::is_native(e)
    {
        return Box::pin(super::white_direct::plan(
            ctx,
            steps,
            wallet,
            now,
            all_approvals,
        ))
        .await;
    }
    if e.family.direct_module() {
        return super::solidly_direct::plan(ctx, steps, wallet, now, all_approvals).await;
    }
    let router = ctx.dex_addr("dex.universal_router", e.family.id)?;
    let permit2 = ctx.dex_addr("dex.permit2", e.family.id)?;
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    let fmt = ur::format(ctx, e)?;
    let o = &first.order;
    // Первый шаг платит роутер (нативная продажа: WBNB из WRAP_ETH) — разрешений кошелька не нужно.
    let approvals = if !first.from_wallet {
        Vec::new()
    } else if all_approvals {
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
        commands.push(chain::routed_command(ctx, s, fmt, wallet, (router, permit2)).await?);
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
