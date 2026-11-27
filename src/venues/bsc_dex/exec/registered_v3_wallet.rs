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
    pub algebra_short_event: bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub algebra_wallet: Option<(Address,B256)>,
    #[serde(default, skip_serializing_if = "standard_event")]
    pub fee_event: bool,
    #[serde(default, skip_serializing_if = "standard_event")]
    pub wallet_module: bool,
    #[serde(default, skip_serializing_if = "standard_event")]
    pub maverick: bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub maverick_chain: Option<u64>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub slipstream: Option<(i32,Address)>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub router02: Option<super::v3_router02::Key>,
}
fn standard_event(extended: &bool) -> bool { !extended }
impl Market {
    pub(in crate::venues::bsc_dex) fn from_entry(ctx:&Ctx,e: &super::super::pools::registry::Entry) -> Option<Self> {
        let algebra=e.family.variant.algebra_router();
        let maverick=e.family.variant==super::super::pools::family_variant::Variant::MaverickTwoRouter;
        let deployer=if algebra{Some(super::super::pools::cypher::deployment(ctx,e).ok()?)}else{None};
        use super::super::pools::family_variant::Variant;
        let abi = ctx.dex_text("dex.v3_wallet_router_abi", e.family.id).ok();
        let router02 = match (abi.as_deref(), e.family.variant) {
            (Some("router02_fee"), Variant::RegisteredUniV3) => Some(super::v3_router02::Key::Fee),
            (Some("router02_spacing"), Variant::RegisteredSpacingV3) => Some(super::v3_router02::Key::Spacing(
                alloy_primitives::aliases::I24::try_from(e.pool.tick_spacing.filter(|s| *s>0)?).ok()?)),
            (Some("classic_spacing"), Variant::RegisteredSlipstream) => Some(super::v3_router02::Key::InlineSpacing(
                alloy_primitives::aliases::I24::try_from(e.pool.tick_spacing.filter(|s| *s>0)?).ok()?)),
            (Some("classic") | None, variant) if !matches!(variant, Variant::RegisteredSpacingV3 | Variant::RegisteredSlipstream) => None,
            _ => return None,
        };
        Some(Self {
            router02,
            maverick_chain: if maverick {Some(ctx.chain_id()?)} else {None},
            slipstream: if e.family.variant==super::super::pools::family_variant::Variant::SlipstreamUniversal { Some((e.pool.tick_spacing.filter(|s| *s>0 && *s<=8388607)?,ctx.dex_addr("dex.permit2",e.family.id).ok()?)) } else {None},
            pool: e.pool.reference.address(), maverick, fee: if algebra || maverick{U24::ZERO}else{U24::from(e.pool.fee_pips?)}, wallet_module:false,algebra_deployer:deployer,
            algebra_wallet:None,algebra_short_event:e.family.variant==Variant::TheoremIntegral,
            fee_event: e.family.variant == super::super::pools::family_variant::Variant::VvsV3,
        })
    }
}
pub trait RouterMarket: Copy {
    fn algebra_wallet(self) -> Option<(Address,B256)> { None }
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
    fn algebra_wallet(self) -> Option<(Address,B256)> { self.algebra_wallet }
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
        if self.algebra_deployer.is_some() && !self.algebra_short_event{return ("Swap(address,address,int256,int256,uint160,uint128,int24,uint24,uint24)",224)}
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
        if let Some(deployer)=self.algebra_deployer && self.wallet_module {
            return super::algebra_wallet::order(self.pool,input,output,deployer,amount,minimum,deadline);
        }
        if let Some(deployer)=self.algebra_deployer{return ICypherRouter::exactInputSingleCall{args:ICypherRouter::Params{tokenIn:input,tokenOut:output,deployer,recipient:wallet,deadline,amountIn:amount,minimum,limit:Default::default()}}.abi_encode().into()}
        if self.wallet_module{return super::super::pools::solidly_v3::execution_data(self.pool,input,output,amount,minimum,deadline)}
        if let Some(key) = self.router02 {
            return key.order(self.fee, (input,output), amount, minimum, wallet, deadline);
        }
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
        if let Some((_,code))=self.market.algebra_wallet() {
            super::algebra_wallet::included(ctx,self.target,code,r).await?;
        }
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
        if let Some((router,_))=self.market.algebra_wallet() {
            return super::algebra_wallet::amounts(self,router,logs);
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

pub(in crate::venues::bsc_dex) async fn module_market(ctx:&Ctx,e:&super::super::pools::registry::Entry,at:Option<&serde_json::Value>)
    -> GatewayResult<(Address,Market)> {
    let bad=||ctx.err(ErrorCode::NoRoute,"Не задан или не подтверждён кошелёк зарегистрированного пула");
    let mut market=Market::from_entry(ctx,e).ok_or_else(bad)?;
    market.wallet_module=true;
    let module=if let Some(at)=at {
        let (module,code)=registered_router::module_binding(ctx,e,at).await?;
        if market.algebra_short_event {market.algebra_wallet=Some((ctx.dex_addr("dex.router",e.family.id)?,code));}
        module
    } else {registered_router::wallet_module(ctx,e).ok_or_else(bad)?};
    Ok((module,market))
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
    if e.family.variant.separate_v3_router()
        && e.family.tradable && selected.is_some() && ctx.dex_addr("dex.router",e.family.id).ok()==selected {
        super::super::pools::registered_v3::fresh(ctx,e,&head.selector()).await?;
        let plan=Box::pin(super::solidly_direct::plan(ctx,std::slice::from_ref(s),wallet,head.timestamp.to::<u64>(),false)).await?;
        if Some(plan.router)!=selected || ctx.settings.generation()!=generation {return Err(fail());}
        return Ok(plan);
    }
    let guarded=selected.is_some() && registered_router::wallet_module(ctx,e)==selected;
    let (target,mut market)=if guarded{
        module_market(ctx,e,Some(&head.selector())).await?
    }else{
        let target=registered_router::checked(ctx,e,o.token_in,o.token_out,&head.selector()).await?;
        (target,Market::from_entry(ctx,e).ok_or_else(fail)?)
    };
    market.wallet_module=guarded;
    if selected != Some(target) || !s.from_wallet || !s.to_wallet {
        return Err(fail());
    }
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
