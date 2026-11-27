//! Финальное сравнение на фактических балансах кошелька: разрешения, обмен,
//! чистый приход, газ каждого вызова. Оба исполнения используют один родительский блок.
use super::super::exec::registered_v3_wallet::RouterMarket;
use super::super::{
    check::{
        sim_base::Base,
        simulate::{self, Call, CallResult},
    },
    ctx::Ctx,
    exec::{
        executor::{abi::ArbExecutor, call::ExecCall},
        fork_swap,
    },
    node::rpc,
    wallet::approve,
};
use super::white_state::Head;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::json;

sol! {interface IMoney {
    function totalSupply() external view returns(uint256);
    function getEthBalance(address owner) external view returns(uint256);
    function balanceOf(address owner) external view returns(uint256);
    function allowance(address owner,address spender) external view returns(uint256);
}}
#[derive(Clone)]
pub(in crate::venues::bsc_dex) struct V5Step {
    pub native: Option<(Address, bool)>,
    pub kind: u8,
    pub pool: Address,
    pub data: Bytes,
}
#[path="v2_funding.rs"]
mod funding;
pub(in crate::venues::bsc_dex) use funding::BurnFunding;

#[derive(Clone)]
pub(in crate::venues::bsc_dex) enum Wire {
    Router02,
    Umbrae(super::super::exec::umbrae::Path),
    RegisteredV3(super::super::exec::registered_v3_wallet::Market),
    SolidlyRouter(super::super::exec::solidly_router_wallet::Market),
    SectorLb(super::super::exec::sectorone::Path),
    Carbon(std::sync::Arc<super::super::exec::carbon_wire::Call>),
    MintClub(super::super::exec::mint_club_wire::Call),
    EkuboV2(super::super::pools::ekubo_v2::OrderKey,Option<Address>),
    DirectModule { pool: Address },
    Bundles(super::super::exec::bundles_wallet::Terms),
    SafeSwap(Address),
    DegenRouter(Address),
    DegenModule(Address),
    Pendle(Address),
    Aktionariat(super::super::exec::aktionariat_wallet::Market),
    BancorLegacy(Box<super::super::exec::bancor_legacy_wallet::Terms>),
    MotoswapPath(super::super::exec::motoswap_wallet::Terms,std::sync::Arc<super::super::exec::motoswap_wallet::Route>),
    FluidHop(super::super::exec::fluid_lite_wallet::Terms,std::sync::Arc<super::super::exec::fluid_lite_hop::Route>),
    FluidDexLite(super::super::exec::fluid_lite_wallet::Terms),
    Ambient(super::super::exec::ambient_wallet::Terms),
    Llamma(super::super::exec::llamma_wallet::Terms),
    LlammaV5(V5Step,Address),
    Motoswap(super::super::exec::motoswap_wallet::Terms),
    Mooniswap(super::super::exec::mooniswap_wallet::Terms),
    DefiPlaza(super::super::exec::defiplaza_wallet::Terms),
    YieldBasis(super::super::exec::yield_basis_wallet::Terms),
    BalancerV1(super::super::exec::balancer_v1_wallet::Terms),
    BancorV3(Box<super::super::exec::bancor_v3_wallet::Terms>),
    UniswapV1(Box<super::super::exec::uniswap_v1_wallet::Terms>),
    Rubicon(Box<super::super::exec::rubicon_wallet::Terms>),
    NativeFlat(super::super::exec::mars_wallet::Terms),
    Prop(Box<super::super::exec::prop_wallet::Terms>),
    Aqua(std::sync::Arc<super::super::exec::aqua_wallet::Terms>),
    Angstrom(std::sync::Arc<super::super::exec::angstrom_wallet::Terms>),
    Stabull(super::super::exec::stabull_wallet::Terms),
    Integral(super::super::exec::integral_wallet::Terms),
    Dagrow(super::super::exec::dagrow_wallet::Terms),
    Temple(std::sync::Arc<super::super::exec::temple_wallet::Key>),
    AntfarmOutput(std::sync::Arc<super::super::exec::antfarm_output_wallet::Terms>),
    Antfarm(std::sync::Arc<super::super::exec::antfarm_wallet::Terms>),
    Vault(B256),
    O1(B256),
    Ranges(super::super::exec::ranges_wallet::Market),
    FluxEth(super::super::exec::flux_eth::Market),
    Vista(super::super::exec::vista_wire::Call),
    Oasis { rebate: bool },
    V5(V5Step),
    AktionariatV5(V5Step,super::super::exec::aktionariat_wallet::Market),
    /// Терминальные держатели входного ERC20, подтверждённые протоколом выбранного шага.
    V5Funded(V5Step, Vec<Address>),
    V5Burn(V5Step, Box<BurnFunding>),
}

impl Wire {
    pub(in crate::venues::bsc_dex) fn funded(step:V5Step,holders:Vec<Address>,burn:Option<BurnFunding>)->Self {
        match burn {
            Some(b)=>Self::V5Burn(step,Box::new(b)),
            None=>Self::V5Funded(step,holders),
        }
    }
}

#[derive(Clone)]
pub(in crate::venues::bsc_dex) struct Order {
    pub payer: Address,
    pub target: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub wire: Wire,
}

pub(in crate::venues::bsc_dex) struct Fill {
    pub fluid_legs: Option<Vec<(U256,U256)>>,
    pub charged: U256,
    pub received: U256,
    pub swap_gas: u64,
    pub approval: (u64, usize),
}
pub(in crate::venues::bsc_dex) async fn measure(ctx: &Ctx, h: Head, o: Order, boundary: bool) -> GatewayResult<Fill> {
    let fail = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "V2: средства, газ или поступление обмена не подтверждены",
        )
    };
    let mut o=o;
    if let Wire::EkuboV2(key,_ )=&mut o.wire {key.amount=i128::try_from(o.amount).ok().filter(|n|*n>0).ok_or_else(fail)?;}
    let tx = |to, input| Call {
        from: o.payer,
        to,
        input,
    };
    let (wrapped, value) = match &o.wire {
        Wire::FluxEth(m)=>(Some(m.wrapped),if o.input==m.wrapped{o.amount}else{U256::ZERO}),
        Wire::BancorLegacy(t)=>(t.native_asset(),t.value(o.input,o.amount)),
        Wire::Mooniswap(t)=>(t.native.then_some(t.wrapped),t.value(o.input,o.amount)),
        Wire::DefiPlaza(t)=>(t.native.then_some(t.wrapped),t.value(o.input,o.amount)),
        Wire::YieldBasis(t)=>(t.native.then_some(t.wrapped),t.value(o.input,o.amount)),
        Wire::BalancerV1(t)=>(t.native.then_some(t.wrapped),t.value(o.input,o.amount)),
        Wire::BancorV3(t)=>(t.native(),t.value(o.amount)),
        Wire::UniswapV1(t)=>(t.native(),t.value(o.amount)),
        Wire::Rubicon(_)=>(None,U256::ZERO),
        Wire::NativeFlat(t)=>(None,t.wei),
        Wire::Carbon(c)=>(c.wrapped(),c.value()),
        Wire::EkuboV2(key,wrapped)=>(*wrapped,key.value()),
        Wire::Vista(v) => (
            if matches!(v.form,super::super::exec::vista_wire::Form::ModuleToken|super::super::exec::vista_wire::Form::SurgeToken) {
                None
            } else {
                Some(v.wrapped)
            },
            v.value(o.amount),
        ),
        Wire::V5(v) | Wire::LlammaV5(v,_) | Wire::AktionariatV5(v,_) | Wire::V5Funded(v,_) | Wire::V5Burn(v,_) => (
            v.native.map(|(w, _)| w),
            if matches!(v.native, Some((_, true))) {
                o.amount
            } else {
                U256::ZERO
            },
        ),
        _ => (None, U256::ZERO),
    };
    let multicall = if wrapped.is_some() {
        Some(ctx.addr("dex.multicall3")?)
    } else {
        None
    };
    let balance = |token| {
        if let Some(mc) = multicall
            && wrapped == Some(token)
        {
            tx(
                mc,
                Bytes::from(IMoney::getEthBalanceCall { owner: o.payer }.abi_encode()),
            )
        } else {
            tx(
                token,
                Bytes::from(IMoney::balanceOfCall { owner: o.payer }.abi_encode()),
            )
        }
    };
    let exact_approval=matches!(&o.wire,Wire::Motoswap(_)|Wire::MotoswapPath(..)|Wire::FluidHop(..)|Wire::FluidDexLite(_)|Wire::Ambient(_)|Wire::Llamma(_)|Wire::Stabull(_)|Wire::Aqua(_)|Wire::Angstrom(_))||matches!(&o.wire,Wire::Aktionariat(_)|Wire::NativeFlat(_)|Wire::BancorLegacy(_)|Wire::Mooniswap(_)|Wire::DefiPlaza(_)|Wire::YieldBasis(_)|Wire::BalancerV1(_)|Wire::BancorV3(_)|Wire::UniswapV1(_)|Wire::Rubicon(_))||matches!(&o.wire,Wire::Prop(t) if t.target==t.router)||matches!(&o.wire, Wire::Integral(t) if t.target==t.relay) || matches!(&o.wire, Wire::Dagrow(t) if t.pool==t.target) || matches!(&o.wire,Wire::Bundles(t) if t.router==t.target);
    let allowed=|token:Address,amount:U256| async move {
        let encoded=Bytes::from(IMoney::allowanceCall{owner:o.payer,spender:o.target}.abi_encode());
        let current:Bytes=rpc::call(ctx,"eth_call",json!([{"to":token,"data":encoded},h.selector()])).await?;
        if current.len()!=32{return Err(fail());}
        if exact_approval {
            approve::fork_exact(ctx,token,o.target,Some(U256::from_be_slice(&current)),amount)
        } else {approve::fork_known(ctx,token,o.target,U256::from_be_slice(&current),amount)}
    };
    let mut approvals=if let Wire::RegisteredV3(m)=&o.wire && let Some((_,permit))=m.slipstream {approve::at_block(ctx,o.input,o.payer,permit,o.target,o.amount,h).await?}else if let Wire::Ranges(m)=&o.wire {super::super::exec::ranges_wallet::permissions(ctx,h,*m,&o).await?}else if let Wire::FluxEth(m)=&o.wire {super::super::exec::flux_eth_plan::approvals(ctx,h,*m,&o).await?}else if wrapped==Some(o.input) || matches!(&o.wire,Wire::Vista(v) if v.form==super::super::exec::vista_wire::Form::SurgeEthOut){Vec::new()}else{
        let cap=match &o.wire {Wire::Carbon(c)=>c.cap(),Wire::MintClub(c) if c.mint=>c.limit,_=>o.amount};
        allowed(o.input,cap).await?
    };
    if let Wire::AntfarmOutput(t)=&o.wire && let Some((token,amount))=t.stock_approval(){
        approvals.extend(allowed(token,amount).await?);
    }
    let mut calls = vec![balance(o.input), balance(o.output)];
    calls.extend(approvals.iter().map(|(a, d)| tx(*a, d.clone())));
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .saturating_add(ctx.int("dex.deadline_s")?),
    );
    let data = calldata(&o, deadline, o.minimum);
    calls.extend([tx(o.target, data), balance(o.input), balance(o.output)]);
    let funding=funding::Inventory::for_order(&o).ok_or_else(fail)?;
    let offset=funding.len();
    calls.splice(0..0,funding.calls(o.payer,o.input));
    calls.extend(funding.calls(o.payer,o.input));
    let values = (!value.is_zero()).then_some((offset + approvals.len() + 2, value));
    let result = frame(ctx, h, &calls, values.as_slice()).await?;
    if result.iter().any(|x| !x.ok()) {
        return Err(fail());
    }
    let n = approvals.len();
    let word =
        |v: &CallResult| (v.return_data.len() == 32).then(|| U256::from_be_slice(&v.return_data));
    let r = &result[offset..result.len() - offset];
    let (before,after)=funding.balances(&result).ok_or_else(fail)?;
    if !funding.confirm(&o,&before,&after,&r[n+2].logs){return Err(fail());}
    let spent = word(&r[0]).and_then(|old| old.checked_sub(word(&r[n + 3])?));
    let received = word(&r[n + 4])
        .and_then(|after| after.checked_sub(word(&r[1])?))
        .ok_or_else(fail)?;
    let exact_target=matches!(&o.wire,Wire::FluidDexLite(t) if t.desired.is_some())||matches!(&o.wire,Wire::Ambient(t) if t.desired.is_some())||matches!(&o.wire,Wire::Llamma(t) if t.desired.is_some())||matches!(&o.wire,Wire::BalancerV1(t) if t.desired.is_some())||matches!(&o.wire,Wire::Stabull(t) if t.desired.is_some())||matches!(&o.wire,Wire::BancorV3(t) if t.desired.is_some())||matches!(&o.wire,Wire::Integral(t) if t.desired.is_some())||matches!(o.wire,Wire::Dagrow(t) if t.desired.is_some());
    let bounded_input=matches!(&o.wire,Wire::Ambient(_)|Wire::Llamma(_)|Wire::LlammaV5(_,_)|Wire::Aktionariat(_)|Wire::AktionariatV5(_,_)) || matches!(&o.wire,Wire::RegisteredV3(m) if m.algebra_wallet.is_some());
    let charged=spent.ok_or_else(fail)?;
    if charged.is_zero() || charged>o.amount || (!exact_target && !bounded_input && charged!=o.amount) || received.is_zero() || received < o.minimum {
        return Err(fail());
    }
    if let Wire::Bundles(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::AktionariatV5(_,m)=&o.wire {
        let mut proof=m.proof(&o,o.minimum,deadline);
        proof.v5=Some(crate::venues::bsc_dex::exec::signed_call::Erc20Call{chain:1,payer:o.payer,module:o.target,calldata:alloy_primitives::keccak256(calldata(&o,deadline,o.minimum))});
        if proof.measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    }
    if let Wire::Aktionariat(m)=&o.wire && m.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::BancorLegacy(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::LlammaV5(step,zero)=&o.wire && super::super::exec::llamma_wallet::measured_v5(&o,step.pool,*zero,&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    let fluid_legs=if let Wire::FluidHop(t,route)=&o.wire {
        let p=route.proof(*t,&o,o.minimum,deadline);
        let log=super::super::exec::signed_call::logs(&r[n+2].logs).ok_or_else(fail)?;
        if p.amounts(&log,o.payer,o.input,o.output)!=Some((charged,received)){return Err(fail());}
        Some(route.flows(&p,&log).ok_or_else(fail)?)
    }else if let Wire::MotoswapPath(t,route)=&o.wire{
        let p=route.proof(*t,&o,o.minimum,deadline);
        let log=super::super::exec::signed_call::logs(&r[n+2].logs).ok_or_else(fail)?;
        if p.amounts(&log,o.payer,o.input,o.output)!=Some((charged,received)){return Err(fail());}
        let mut flows=route.flows(&p,&log).ok_or_else(fail)?;
        flows.first_mut().ok_or_else(fail)?.0=charged;
        flows.last_mut().ok_or_else(fail)?.1=received;
        Some(flows)
    }else{None};
    if let Wire::FluidDexLite(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::Ambient(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::Llamma(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::Motoswap(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::Mooniswap(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::DefiPlaza(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::YieldBasis(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::BalancerV1(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::BancorV3(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::UniswapV1(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::Rubicon(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::NativeFlat(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::Prop(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::Aqua(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::Angstrom(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::Stabull(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::Integral(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::Dagrow(t)=&o.wire && t.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::SolidlyRouter(market)=&o.wire {
        let proof=super::super::exec::solidly_router_wallet::Proof{market:*market,target:o.target,payer:o.payer,input:o.input,output:o.output,budget:o.amount,minimum:o.minimum,deadline};
        if proof.measured(&r[n+2].logs)!=Some((o.amount,received)){return Err(fail());}
    }
    if let Wire::Ranges(m)=&o.wire && m.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    if let Wire::RegisteredV3(market)=&o.wire {
        let proof=super::super::exec::registered_v3_wallet::Proof{market:*market,target:o.target,payer:o.payer,input:o.input,output:o.output,budget:o.amount,minimum:o.minimum,deadline};
        if proof.measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail());}
    }
    if let Wire::Carbon(c)=&o.wire {
        let proof=super::super::exec::carbon_fact::Proof{call:(**c).clone(),payer:o.payer};
        if proof.measured(&r[n+2].logs)!=Some((o.amount,received)){return Err(fail());}
    }
    if let Wire::AntfarmOutput(t)=&o.wire {
        let proof=super::super::exec::antfarm_output_wallet::Proof{runtime:None,terms:(**t).clone(),payer:o.payer,minimum:o.minimum,deadline};
        if proof.measured(&r[n+2].logs)!=Some((o.amount,received)){return Err(fail());}
    }
    if let Wire::Antfarm(t)=&o.wire {
        let proof=super::super::exec::antfarm_wallet::Proof{runtime:None,terms:(**t).clone(),payer:o.payer,minimum:o.minimum,deadline};
        if proof.measured(&r[n+2].logs)!=Some((o.amount,received)){return Err(fail());}
    }
    if let Wire::FluxEth(m)=&o.wire && m.proof(&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((o.amount,received)) {return Err(fail());}
    if let Wire::O1(id)=&o.wire {
        let proof=super::super::exec::o1_wallet::Proof{id:*id,module:o.target,payer:o.payer,input:o.input,output:o.output,amount:o.amount,minimum:o.minimum,deadline};
        if proof.measured(&r[n+2].logs)!=Some((o.amount,received)){return Err(fail());}
    }
    if let Wire::SafeSwap(m)|Wire::DegenRouter(m)|Wire::DegenModule(m)=&o.wire && super::super::exec::safeswap_wallet::Proof::order(*m,&o,o.minimum,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::Pendle(m)=&o.wire && super::super::exec::pendle_wallet::Proof::order(*m,&o,deadline).measured(&r[n+2].logs)!=Some((charged,received)){return Err(fail())}
    if let Wire::Temple(key)=&o.wire {
        let proof=super::super::exec::temple_wallet::Proof{key:**key,payer:o.payer,input:o.input,output:o.output,amount:o.amount,minimum:o.minimum,deadline};
        if proof.measured(&r[n+2].logs)!=Some((o.amount,received)){return Err(fail());}
    }
    if let Wire::MintClub(c)=&o.wire {
        let proof=super::super::exec::mint_club_fact::Proof{call:*c,bond:c.protection.map_or(o.target,|v|v.bond),payer:o.payer};
        if proof.measured(&r[n+2].logs)!=Some((o.amount,received)){return Err(fail());}
    }
    if boundary {
        // Тот же плательщик, исходные остатки и разрешения. Более строгий лишь минимум.
        let strict = received.checked_add(U256::ONE).ok_or_else(fail)?;
        let strict_carbon=match &o.wire{Wire::Carbon(c)=>Some(c.tighten(o.amount,received).ok_or_else(fail)?),_=>None};
        calls[offset + n + 2].input = match &o.wire {
            Wire::FluidDexLite(t) if exact_target=>{let mut p=t.proof(&o,o.minimum,deadline);p.maximum=charged.checked_sub(U256::ONE).ok_or_else(fail)?;p.data()}
            Wire::Ambient(t) if exact_target=>{let mut p=t.proof(&o,o.minimum,deadline);p.maximum=charged.checked_sub(U256::ONE).ok_or_else(fail)?;p.data()}
            Wire::Llamma(t) if exact_target=>{let mut p=t.proof(&o,o.minimum,deadline);p.maximum=charged.checked_sub(U256::ONE).ok_or_else(fail)?;p.data()}
            Wire::BalancerV1(t) if exact_target=>{let mut p=t.proof(&o,o.minimum,deadline);p.maximum=charged.checked_sub(U256::ONE).ok_or_else(fail)?;p.data()}
            Wire::BancorV3(t) if exact_target=>{let mut p=t.proof(&o,o.minimum,deadline);p.maximum=charged.checked_sub(U256::ONE).ok_or_else(fail)?;p.data()}
            Wire::Stabull(t) if exact_target=>{let mut p=t.proof(&o,o.minimum,deadline);p.maximum=charged.checked_sub(U256::ONE).ok_or_else(fail)?;p.data()}
            Wire::Integral(t) if exact_target=>{let mut p=t.proof(&o,o.minimum,deadline);p.amount=charged.checked_sub(U256::ONE).ok_or_else(fail)?;p.data()}
            Wire::Dagrow(t) if exact_target=>{
                let mut proof=t.proof(&o,o.minimum,deadline);proof.amount=charged.checked_sub(U256::ONE).ok_or_else(fail)?;proof.data()
            }
            Wire::Carbon(_)=>strict_carbon.as_ref().ok_or_else(fail)?.data(),
            Wire::MintClub(c)=>c.tighten(spent.ok_or_else(fail)?,received).ok_or_else(fail)?.data(o.payer),
            _=>calldata(&o, deadline, strict),
        };
        let strict_value=match &o.wire{Wire::BalancerV1(t) if exact_target=>t.value(o.input,charged.checked_sub(U256::ONE).ok_or_else(fail)?), Wire::BancorV3(t) if exact_target=>t.value(charged.checked_sub(U256::ONE).ok_or_else(fail)?),_=>strict_carbon.as_ref().map_or(value,|c|c.value())};
        let stricter=(!strict_value.is_zero()).then_some((offset+n+2,strict_value));
        let strict_result = frame(ctx, h, &calls, stricter.as_slice()).await?;
        let alt = &strict_result[offset..strict_result.len() - offset];
        let (strict_before,strict_after)=funding.balances(&strict_result).ok_or_else(fail)?;
        // При более строгом минимуме LambdaClass может исполнить публичную заявку через второй рынок.
        let improved=if let Wire::Prop(t)=&o.wire && t.fallback_allowed && alt[n+2].ok() {
            let p=t.proof(&o,strict,deadline);
            let paid=word(&alt[0]).and_then(|a|a.checked_sub(word(&alt[n+3])?));
            let net=word(&alt[n+4]).and_then(|a|a.checked_sub(word(&alt[1])?));
            alt.iter().all(|v|v.ok()) && paid==Some(o.amount) && net.is_some_and(|v|v>=strict)
                && paid.zip(net)==p.measured(&alt[n+2].logs) && strict_before==before && strict_after==before
        }else{false};
        if !improved && (strict_before!=before || strict_after!=before
            || alt[n + 2].ok()
            || alt[n + 2].error.is_none()
            || alt.iter().enumerate().any(|(j, v)| j != n + 2 && !v.ok())
            || word(&alt[0]) != word(&r[0])
            || word(&alt[1]) != word(&r[1])
            || word(&alt[n + 3]) != word(&r[0])
            || word(&alt[n + 4]) != word(&r[1]))
        {
            return Err(fail());
        }
    }
    let approve_gas = r[2..2 + n].iter().map(|x| x.gas_used.to::<u64>()).sum();
    Ok(Fill {
        fluid_legs,
        charged,
        received,
        swap_gas: r[n + 2].gas_used.to(),
        approval: (approve_gas, n),
    })
}

fn calldata(o: &Order, deadline: U256, minimum: U256) -> Bytes {
    match &o.wire {
        Wire::Carbon(c)=>c.data(),
        Wire::MintClub(c) => c.data(o.payer),
        Wire::AntfarmOutput(t)=>t.data(minimum,deadline),
        Wire::Antfarm(t)=>t.data(minimum,deadline),
        Wire::Bundles(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Aktionariat(m)=>m.proof(o,minimum,deadline).data(),
        Wire::BancorLegacy(t)=>t.proof(o,minimum,deadline).data(),
        Wire::MotoswapPath(t,route)=>route.proof(*t,o,minimum,deadline).data(),
        Wire::FluidHop(t,route)=>route.proof(*t,o,minimum,deadline).data(),
        Wire::FluidDexLite(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Ambient(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Llamma(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Motoswap(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Mooniswap(t)=>t.proof(o,minimum,deadline).data(),
        Wire::DefiPlaza(t)=>t.proof(o,minimum,deadline).data(),
        Wire::YieldBasis(t)=>t.proof(o,minimum,deadline).data(),
        Wire::BalancerV1(t)=>t.proof(o,minimum,deadline).data(),
        Wire::BancorV3(t)=>t.proof(o,minimum,deadline).data(),
        Wire::UniswapV1(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Rubicon(t)=>t.proof(o,minimum,deadline).data(),
        Wire::NativeFlat(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Prop(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Aqua(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Angstrom(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Stabull(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Integral(t)=>t.proof(o,minimum,deadline).data(),
        Wire::Dagrow(t)=>t.proof(o,minimum,deadline).data(),
        Wire::SafeSwap(m)|Wire::DegenRouter(m)|Wire::DegenModule(m)=>super::super::exec::safeswap_wallet::Proof::order(*m,o,minimum,deadline).data(),
        Wire::Pendle(m)=>super::super::exec::pendle_wallet::data(*m,o.input,o.output,o.amount,minimum,deadline),
        Wire::Temple(k)=>k.data(o.input,o.output,o.amount,minimum,o.payer,deadline),
        Wire::EkuboV2(key,_) => key.calldata(minimum,deadline),
        Wire::FluxEth(m)=>m.proof(o,minimum,deadline).data(),
        Wire::O1(id) => super::super::exec::o1_wallet::data(*id,o.input,o.output,o.amount,minimum,deadline),
        Wire::Vault(id) => super::super::exec::vault_direct::data(
            *id, o.input, o.output, o.amount, minimum, o.payer, deadline,
        ),
        Wire::Vista(v) => v.data(o.input, o.output, o.amount, minimum, o.payer, deadline),
        Wire::V5(v5) | Wire::LlammaV5(v5,_) | Wire::AktionariatV5(v5,_) | Wire::V5Funded(v5,_) | Wire::V5Burn(v5,_) => {
            let step = ArbExecutor::Step {
                kind: v5.kind,
                pool: v5.pool,
                tokenIn: o.input,
                tokenOut: o.output,
                shareBps: 10000,
                data: v5.data.clone(),
            };
            ExecCall {
                executor: o.target,
                token_in: o.input,
                amount_in: o.amount,
                token_out: o.output,
                min_out: minimum,
                deadline,
                steps: vec![step],
                native: v5.native,
                gated: vec![],
            }
            .data()
        }
        Wire::DirectModule { pool } => super::super::pools::solidly_v3::execution_data(
            *pool, o.input, o.output, o.amount, minimum, deadline,
        ),
        Wire::Oasis { rebate } => super::super::exec::oasis_direct::swap_data(
            o.input, o.output, o.amount, minimum, o.payer, deadline, *rebate,
        ),
        Wire::SolidlyRouter(market) => market.order(o.input,o.output,o.amount,minimum,o.payer,deadline),
        Wire::Ranges(m)=>m.order(o.input,o.output,o.amount,minimum,o.payer,deadline),
        Wire::RegisteredV3(market) => market.order(o.input,o.output,o.amount,minimum,o.payer,deadline),
        Wire::Umbrae(path) => path.data(o.input,o.output,o.amount,minimum,o.payer,deadline),
        Wire::SectorLb(path) => path.data(o.input,o.output,o.amount,minimum,o.payer,deadline),
        Wire::Router02 => {
            fork_swap::swap_data(o.amount, minimum, &[o.input, o.output], o.payer, deadline)
        }
    }
}

async fn frame(
    ctx: &Ctx,
    h: Head,
    calls: &[Call],
    values: &[(usize, U256)],
) -> GatewayResult<Vec<CallResult>> {
    let mut req = simulate::params(
        calls,
        values,
        &[],
        Base::for_chain(ctx.chain_id(), h.number.to(), h.timestamp.to()),
    );
    req[1] = h.selector();
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Frame {
        parent_hash: B256,
        calls: Vec<CallResult>,
    }
    let frames: Vec<Frame> = rpc::call(ctx, "eth_simulateV1", req).await?;
    let [frame]: [Frame; 1] = frames
        .try_into()
        .map_err(|_| ctx.err(ErrorCode::BadResponse, "Изменилось число блоков симуляции"))?;
    if frame.parent_hash != h.hash || frame.calls.len() != calls.len() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Изменился блок или число откликов симуляции",
        ));
    }
    Ok(frame.calls)
}
#[cfg(test)]
#[path="native_ranges_tests.rs"]
mod native_ranges_tests;
