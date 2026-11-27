//! Signed bounds and wallet accounting for Antfarm fees bought with the proceeds of a sale.
use super::super::{
    ctx::Ctx,
    math::antfarm_output::{Funds, OutputFill},
    node::{call, rpc},
    pools::{antfarm_book, registry::Entry, solidly_classic_factory},
    quote::{v2_wallet_sim, white_state::Head},
    send::track::Receipt,
    wallet::approve,
};
use super::{
    chain::Step,
    fill,
    plan::{DirectCall, Plan},
    signed_call,
};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
};
use alloy::{rpc::types::Log, sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

sol! {
    struct Bound {
        address input;address output;uint16 tier;uint16 fundingTier;bool supporting;bool useLoan;
        uint256 amount;uint256 minCredit;uint256 maxAtf;uint256 maxFunding;uint256 maxLoanFee;uint256 deadline;
    }
    function execute(Bound order);
    interface Funding {
        function balanceOf(address owner) external view returns(uint256);
        function allowance(address owner,address spender) external view returns(uint256);
        function getProtocolFeesCollector() external view returns(address);
        function getFlashLoanFeePercentage() external view returns(uint256);
        function getPausedState() external view returns(bool paused,uint256 pauseEnd,uint256 bufferEnd);
    }
}

pub(in crate::venues::bsc_dex) fn selector() -> [u8; 4] {
    executeCall::SELECTOR
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Terms {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atf: Option<Address>,
    pub module: Address,
    pub input: Address,
    pub output: Address,
    pub tier: u16,
    pub funding: u16,
    pub use_loan: bool,
    pub amount: U256,
    pub max_atf: U256,
    pub max_funding: U256,
    pub max_loan_fee: U256,
}
impl Terms {
    pub(in crate::venues::bsc_dex) fn new(
        module: (Address, Address),
        input: Address,
        output: Address,
        tier: u16,
        funding_tier: u16,
        use_loan: bool,
        fill: &OutputFill,
    ) -> Self {
        Self {
            atf: Some(module.1),
            module: module.0,
            input,
            output,
            tier,
            funding: funding_tier,
            use_loan,
            amount: fill.primary.debit,
            max_atf: fill.primary.separate_atf,
            max_funding: fill.refill.debit,
            max_loan_fee: fill.loan_fee,
        }
    }
    pub(in crate::venues::bsc_dex) fn stock_approval(&self) -> Option<(Address, U256)> {
        if self.use_loan {
            None
        } else {
            Some((self.atf?, self.max_atf))
        }
    }
    pub fn data(&self, minimum: U256, deadline: U256) -> Bytes {
        executeCall {
            order: Bound {
                input: self.input,
                output: self.output,
                tier: self.tier,
                fundingTier: self.funding,
                supporting: false,
                useLoan: self.use_loan,
                amount: self.amount,
                minCredit: minimum,
                maxAtf: self.max_atf,
                maxFunding: self.max_funding,
                maxLoanFee: self.max_loan_fee,
                deadline,
            },
        }
        .abi_encode()
        .into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<B256>,
    pub terms: Terms,
    pub payer: Address,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub fn data(&self) -> Bytes {
        self.terms.data(self.minimum, self.deadline)
    }
    pub(crate) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.terms.module,
            U256::ZERO,
            self.data(),
        )
        .await?;
        super::super::pools::antfarm_profile::inclusion(
            ctx,
            r.block_hash,
            self.terms.module,
            self.terms.atf,
            self.runtime,
        )
        .await
    }
    pub(in crate::venues::bsc_dex) fn measured(
        &self,
        rows: &[super::super::check::simulate::SimLog],
    ) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(rows)?,
            self.payer,
            self.terms.input,
            self.terms.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let t = &self.terms;
        let atf = t.atf?;
        if (wallet, input, output) != (self.payer, t.input, t.output)
            || self.minimum.is_zero()
            || self.deadline.is_zero()
            || wallet.is_zero()
            || t.module.is_zero()
            || wallet == t.module
            || input == output
            || [input, output].contains(&atf)
            || t.amount.is_zero()
            || t.max_atf.is_zero()
            || t.max_funding.is_zero()
        {
            return None;
        }
        let topics = [
            keccak256("Settled(address,address,address,uint256,uint256,uint256,uint256,uint256)"),
            wallet.into_word(),
            input.into_word(),
            output.into_word(),
        ];
        let mut matched = logs
            .iter()
            .filter(|l| l.address() == t.module && l.topics() == topics);
        let data = &matched.next()?.data().data;
        if matched.next().is_some() || data.len() != 160 {
            return None;
        }
        let word = |idx: usize| U256::from_be_slice(&data[idx * 32..(idx + 1) * 32]);
        let incoming = |token| fill::moved_logs(logs, token, None, Some(wallet));
        let outgoing = |token| fill::moved_logs(logs, token, Some(wallet), None);
        let spent = outgoing(input).checked_sub(incoming(input))?;
        let received = incoming(output).checked_sub(outgoing(output))?;
        let atf_credit = incoming(atf).checked_sub(outgoing(atf))?;
        (spent == t.amount
            && spent == word(0)
            && received >= self.minimum
            && received == word(1)
            && word(2) == atf_credit
            && (t.use_loan || atf_credit.is_zero())
            && word(3) <= t.max_funding
            && word(4) <= t.max_loan_fee
            && (t.use_loan || word(4).is_zero()))
        .then_some((spent, received))
    }
}
fn unavailable(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Antfarm: не подтверждены источник комиссии, денежный предел или выбранный модуль",
    )
}
async fn view<C: SolCall>(ctx: &Ctx, to: Address, c: C, h: Head) -> GatewayResult<C::Return> {
    let data = Bytes::from(c.abi_encode());
    let raw: Bytes =
        rpc::call(ctx, "eth_call", json!([{"to":to,"data":data},h.selector()])).await?;
    call::decode::<C>(&raw)
}
pub(in crate::venues::bsc_dex) async fn module(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
) -> GatewayResult<Address> {
    if !antfarm_book_market(e) {
        return Err(unavailable(ctx));
    }
    super::super::pools::antfarm_profile::module(ctx, e.family.id, &h.selector(), true)
        .await
        .map(|(a, _)| a)
}
fn antfarm_book_market(e: &Entry) -> bool {
    matches!(e.pool.state, PoolState::Antfarm(_))
}
pub(in crate::venues::bsc_dex) async fn funds(
    ctx: &Ctx,
    id: &str,
    payer: Address,
    output: Address,
    h: Head,
    use_loan: bool,
) -> GatewayResult<Funds> {
    let atf = ctx.dex_addr("dex.antfarm_fee_token", id)?;
    if atf.is_zero() || payer.is_zero() || output.is_zero() || output == atf {
        return Err(unavailable(ctx));
    }
    if !use_loan {
        let atf = view(ctx, atf, Funding::balanceOfCall { owner: payer }, h).await?;
        return Ok(Funds::Wallet { atf });
    }
    let (vault, vault_hash) = super::super::pools::antfarm_profile::vault(ctx, id)?;
    let tag = h.selector();
    let (_, paused, collector, output_balance) = tokio::try_join!(
        solidly_classic_factory::bind(ctx, vault, vault_hash, None, &tag),
        view(ctx, vault, Funding::getPausedStateCall {}, h),
        view(ctx, vault, Funding::getProtocolFeesCollectorCall {}, h),
        view(ctx, output, Funding::balanceOfCall { owner: vault }, h)
    )?;
    if paused.paused || collector.is_zero() || output_balance.is_zero() {
        return Err(unavailable(ctx));
    }
    let fee_per_1e18 = view(ctx, collector, Funding::getFlashLoanFeePercentageCall {}, h).await?;
    Ok(Funds::Vault {
        output_balance,
        fee_per_1e18,
    })
}

pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selected: Option<Address>,
    funding_tier: u16,
    use_loan: bool,
) -> GatewayResult<Plan> {
    if !s.from_wallet || !s.to_wallet || !antfarm_book_market(&s.entry) {
        return Err(unavailable(ctx));
    }
    let generation = ctx.settings.generation();
    let e = &s.entry;
    let o = &s.order;
    let h = super::super::quote::white_state::head(ctx, &format!("{height:#x}")).await?;
    let tag = h.selector();
    let ((target, runtime), source, book) = tokio::try_join!(
        super::super::pools::antfarm_profile::module(ctx, e.family.id, &tag, true),
        funds(ctx, e.family.id, payer, o.token_out, h, use_loan),
        antfarm_book::read(
            ctx,
            e.family,
            e.pool.reference.address(),
            (e.coin, e.usdt),
            h
        )
    )?;
    if Some(target) != selected
        || !book.market.tokens.contains(&o.token_out)
        || o.token_in == o.token_out
    {
        return Err(unavailable(ctx));
    }
    let paid = book
        .paid_from_output(o.token_in, o.amount_in, funding_tier, source)
        .filter(|f| f.credit >= o.min_out)
        .ok_or_else(|| unavailable(ctx))?;
    let terms = Terms::new(
        (target, book.market.atf),
        o.token_in,
        o.token_out,
        book.market.tier,
        funding_tier,
        use_loan,
        &paid,
    );
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| unavailable(ctx))?,
    );
    let proof = Proof {
        runtime: Some(runtime),
        terms: terms.clone(),
        payer,
        minimum: o.min_out,
        deadline,
    };
    let measure = v2_wallet_sim::Order {
        target,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::AntfarmOutput(Arc::new(terms.clone())),
    };
    let mut approvals = Vec::new();
    let required = std::iter::once((o.token_in, o.amount_in))
        .chain(terms.stock_approval())
        .collect::<Vec<_>>();
    for (token, amount) in required {
        let allowed = view(
            ctx,
            token,
            Funding::allowanceCall {
                owner: payer,
                spender: target,
            },
            h,
        )
        .await?;
        approvals.extend(approve::fork_known(ctx, token, target, allowed, amount)?);
    }
    v2_wallet_sim::measure(ctx, h, measure, true).await?;
    if generation != ctx.settings.generation() {
        return Err(unavailable(ctx));
    }
    let mut plan = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    plan.set_direct_market(DirectCall::AntfarmOutput(Arc::new(proof)));
    Ok(plan)
}

#[cfg(test)]
#[path = "antfarm_output_wallet_tests.rs"]
mod tests;
