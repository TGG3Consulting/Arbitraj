//! Поручение штатного V3 SwapRouter, подписанный расход, минимум, срок и проверка после включения.
use super::{
    chain::Step,
    fill,
    plan::{DirectCall, Plan},
    signed_call, white_direct,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        pools::registered_router,
        quote::{v2_wallet_sim, white_state},
        send::track::Receipt,
    },
};
use alloy::{rpc::types::Log, sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, aliases::U24, keccak256};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

sol! {interface IV3Router {
    struct ExactIn { address tokenIn;address tokenOut;uint24 fee;address recipient;uint256 deadline;uint256 amountIn;uint256 amountOutMinimum;uint160 sqrtPriceLimitX96; }
    function exactInputSingle(ExactIn params) external payable returns(uint256 amountOut);
}}
sol!{interface ICypherRouter{
    struct Params {address tokenIn;address tokenOut;address deployer;address recipient;uint256 deadline;uint256 amountIn;uint256 minimum;uint160 limit;}
    function exactInputSingle(Params calldata args) external payable returns(uint256);
}}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Market {
    pub pool: Address,
    pub fee: U24,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub algebra_deployer:Option<Address>,
    #[serde(default, skip_serializing_if = "standard_event")]
    pub fee_event: bool,
    #[serde(default, skip_serializing_if = "standard_event")]
    pub wallet_module: bool,
    #[serde(default, skip_serializing_if = "standard_event")]
    pub maverick: bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub slipstream: Option<(i32,Address)>,
}
fn standard_event(extended: &bool) -> bool { !extended }
impl Market {
    pub(in crate::venues::bsc_dex) fn from_entry(ctx:&Ctx,e: &super::super::pools::registry::Entry) -> Option<Self> {
        let algebra=e.family.variant==super::super::pools::family_variant::Variant::CypherIntegral;
        let maverick=e.family.variant==super::super::pools::family_variant::Variant::MaverickTwoRouter;
        let deployer=if algebra{Some(super::super::pools::cypher::deployment(ctx,e).ok()?)}else{None};
        Some(Self {
            slipstream: if e.family.variant==super::super::pools::family_variant::Variant::SlipstreamUniversal { Some((e.pool.tick_spacing.filter(|s| *s>0 && *s<=8388607)?,ctx.dex_addr("dex.permit2",e.family.id).ok()?)) } else {None},
            pool: e.pool.reference.address(), maverick, fee: if algebra || maverick{U24::ZERO}else{U24::from(e.pool.fee_pips?)}, wallet_module:false,algebra_deployer:deployer,
            fee_event: e.family.variant == super::super::pools::family_variant::Variant::VvsV3,
        })
    }
}
pub trait RouterMarket: Copy {
    fn emitter(self) -> Address;
    fn topics(self, actual: &[B256], signature: B256, target:Address, payer:Address, _input:Address, _output:Address) -> bool {
        actual == [signature, target.into_word(), payer.into_word()]
    }
    fn parties(self, _data: &[u8], _router: Address, _wallet: Address) -> bool {true}
    fn event(self) -> (&'static str, usize);
    fn matches(self, data: &[u8], side: usize, spent: U256, received: U256, budget: U256) -> bool;
    fn order(
        self,
        input: Address,
        output: Address,
        amount: U256,
        minimum: U256,
        wallet: Address,
        deadline: U256,
    ) -> Bytes;
}
impl RouterMarket for Market {
    fn emitter(self) -> Address {
        self.pool
    }
    fn topics(self, actual: &[B256], signature: B256, router: Address, wallet: Address, _input: Address, _output: Address) -> bool {
        if self.maverick { actual == [signature] } else {actual == [signature,router.into_word(),wallet.into_word()]}
    }
    fn parties(self, data: &[u8], router: Address, wallet: Address) -> bool {
        !self.maverick || super::maverick_two_wire::parties(data,router,wallet)
    }
    fn event(self) -> (&'static str, usize) {
        if self.maverick {return super::maverick_two_wire::EVENT;}
        if self.algebra_deployer.is_some(){return ("Swap(address,address,int256,int256,uint160,uint128,int24,uint24,uint24)",224)}
        if self.fee_event {
            return ("Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)",224);
        }
        (
            "Swap(address,address,int256,int256,uint160,uint128,int24)",
            160,
        )
    }
    fn matches(self, data: &[u8], side: usize, spent: U256, received: U256, budget: U256) -> bool {
        if self.maverick {return super::maverick_two_wire::amounts(data, side, spent, received, budget);}
        let word = |i: usize| U256::from_be_slice(&data[32 * i..32 * (i + 1)]);
        (!self.wallet_module || spent==budget) && spent <= budget
            && !spent.bit(255)
            && !received.bit(255)
            && word(side) == spent
            && word(1 - side) == U256::ZERO.wrapping_sub(received)
    }
    fn order(
        self,
        input: Address,
        output: Address,
        amount: U256,
        minimum: U256,
        wallet: Address,
        deadline: U256,
    ) -> Bytes {
        if self.maverick {return super::maverick_two_wire::order(self.pool,input<output,amount,minimum,wallet,deadline);}
        if let Some((spacing,_))=self.slipstream {
            use alloy::sol_types::SolValue;
            let mut path=Vec::with_capacity(43);
            path.extend_from_slice(input.as_slice()); path.extend_from_slice(&spacing.to_be_bytes()[1..]); path.extend_from_slice(output.as_slice());
            let params=(wallet,amount,minimum,Bytes::from(path),true).abi_encode_params();
            return super::ur::execute(vec![(super::ur::V3_SWAP_EXACT_IN,Bytes::from(params))],deadline)
        }
        if let Some(deployer)=self.algebra_deployer{return ICypherRouter::exactInputSingleCall{args:ICypherRouter::Params{tokenIn:input,tokenOut:output,deployer,recipient:wallet,deadline,amountIn:amount,minimum,limit:Default::default()}}.abi_encode().into()}
        if self.wallet_module{return super::super::pools::solidly_v3::execution_data(self.pool,input,output,amount,minimum,deadline)}
        IV3Router::exactInputSingleCall {
            params: IV3Router::ExactIn {
                tokenIn: input,
                tokenOut: output,
                fee: self.fee,
                recipient: wallet,
                deadline,
                amountIn: amount,
                amountOutMinimum: minimum,
                sqrtPriceLimitX96: Default::default(),
            },
        }
        .abi_encode()
        .into()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof<M = Market> {
    pub market: M,
    pub target: Address,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub budget: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl<M: RouterMarket> Proof<M> {
    pub(super) fn data(&self) -> Bytes {
        self.market.order(
            self.input,
            self.output,
            self.budget,
            self.minimum,
            self.payer,
            self.deadline,
        )
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.target,
            U256::ZERO,
            self.data(),
        )
        .await
    }
    pub(in crate::venues::bsc_dex) fn measured(
        &self,
        logs: &[crate::venues::bsc_dex::check::simulate::SimLog],
    ) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(logs)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (wallet, input, output) != (self.payer, self.input, self.output)
            || wallet.is_zero()
            || input == output
            || self.minimum.is_zero()
        {
            return None;
        }
        let (signature, bytes) = self.market.event();
        let swap = keccak256(signature);
        let mut hits = logs
            .iter()
            .filter(|l| l.address() == self.market.emitter() && l.topics().first() == Some(&swap));
        let event = hits.next()?;
        if hits.next().is_some()
            || !self.market.topics(event.topics(),swap,self.target,wallet,input,output)
            || event.data().data.len() != bytes
            || !self.market.parties(&event.data().data,self.target,wallet)
        {
            return None;
        }
        let i = usize::from(input > output);
        let spent = fill::moved_logs(logs, input, Some(wallet), None)
            .checked_sub(fill::moved_logs(logs, input, None, Some(wallet)))?;
        let received = fill::moved_logs(logs, output, None, Some(wallet))
            .checked_sub(fill::moved_logs(logs, output, Some(wallet), None))?;
        (spent > U256::ZERO
            && received >= self.minimum
            && self
                .market
                .matches(&event.data().data, i, spent, received, self.budget))
        .then_some((spent, received))
    }
}

pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    wallet: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "V3 Router: адрес оплаты, токены или границы поручения не подтверждены",
        )
    };
    let generation = ctx.settings.generation();
    let e = &s.entry;
    let o = &s.order;
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    if e.family.variant == super::super::pools::family_variant::Variant::VvsV3
        && e.family.tradable && selected.is_some() && ctx.dex_addr("dex.router",e.family.id).ok()==selected {
        super::super::pools::registered_v3::fresh(ctx,e,&head.selector()).await?;
        let plan=Box::pin(super::solidly_direct::plan(ctx,std::slice::from_ref(s),wallet,head.timestamp.to::<u64>(),false)).await?;
        if Some(plan.router)!=selected || ctx.settings.generation()!=generation {return Err(fail());}
        return Ok(plan);
    }
    let guarded=selected.is_some() && registered_router::wallet_module(ctx,e)==selected;
    let target=if guarded{registered_router::checked_module(ctx,e,&head.selector()).await?}else{
        registered_router::checked(ctx, e, o.token_in, o.token_out, &head.selector()).await?
    };
    if selected != Some(target) || !s.from_wallet || !s.to_wallet {
        return Err(fail());
    }
    let mut market = Market::from_entry(ctx,e).ok_or_else(fail)?;market.wallet_module=guarded;
    let now = head.timestamp.to::<u64>();
    let proof = Proof {
        market,
        target,
        payer: wallet,
        input: o.token_in,
        output: o.token_out,
        budget: o.amount_in,
        minimum: o.min_out,
        deadline: U256::from(
            now.checked_add(ctx.int("dex.deadline_s")?)
                .ok_or_else(fail)?,
        ),
    };
    let order = v2_wallet_sim::Order {
        payer: wallet,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::RegisteredV3(market),
    };
    v2_wallet_sim::measure(ctx, head, order, true).await?;
    let mut p = if let Some((_,permit))=market.slipstream {
        let auth=super::super::wallet::approve::at_block(ctx,o.token_in,wallet,permit,target,o.amount_in,head).await?;
        if generation!=ctx.settings.generation(){return Err(fail());}
        Plan::new(ctx,(auth,target,wallet),o.token_out,Vec::new(),o.min_out,proof.deadline)?
    } else {white_direct::build(ctx,s,wallet,target,now,false,generation).await?};
    p.set_direct_market(DirectCall::RegisteredV3(Arc::new(proof)));
    Ok(p)
}

#[cfg(test)]
#[path = "registered_v3_receipt_tests.rs"]
mod tests;
