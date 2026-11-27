//! Подписанное поручение Stabull. Квитанция связывает исходный расход с конкретной Curve и выплатой владельцу.
use super::{fill, signed_call};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, quote::v2_wallet_sim, send::track::Receipt},
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

alloy::sol! {
    interface IPool {
        function originSwap(address a,address b,uint256 spend,uint256 minimum,uint256 deadline) external returns(uint256);
        function targetSwap(address a,address b,uint256 maximum,uint256 desired,uint256 deadline) external returns(uint256);
        event Trade(address indexed trader,address indexed origin,address indexed target,uint256 originAmount,uint256 targetAmount,int128 rawProtocolFee);
    }
    interface IRouter {function originSwap(address quote,address a,address b,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);}
    interface IModule {
        function execute(address pool,address a,address b,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
        function executeTarget(address pool,address a,address b,uint256 maximum,uint256 desired,uint256 deadline) external returns(uint256,uint256);
        event Filled(address indexed payer,address indexed recipient,address indexed pool,address input,address output,uint256 paid,uint256 received);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Form {
    Curve,
    Router,
    Module,
    Euler{v2:bool,guarded:bool,vaults:[Address;2]},
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Terms {
    pub form: Form,
    pub pool: Address,
    pub quote: Address,
    pub target: Address,
    pub desired: Option<U256>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub maximum: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Terms {
    pub(in crate::venues::bsc_dex) fn proof(
        self,
        o: &v2_wallet_sim::Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            terms: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            maximum: o.amount,
            minimum,
            deadline,
        }
    }
}
impl Proof {
    pub(crate) fn data(&self) -> Bytes {
        let Self {
            terms: t,
            payer: _,
            input: a,
            output: b,
            maximum,
            minimum,
            deadline,
        } = *self;
        let desired = t.desired.unwrap_or_default().max(minimum);
        match (t.form, t.desired.is_some()) {
            (Form::Euler{guarded:false,..},_)=>return super::euler_wallet::calldata(self),
            (Form::Module | Form::Euler{guarded:true,..}, true) => IModule::executeTargetCall {
                pool: t.pool,
                a,
                b,
                maximum,
                desired,
                deadline,
            }
            .abi_encode(),
            (Form::Module | Form::Euler{guarded:true,..}, false) => IModule::executeCall {
                pool: t.pool,
                a,
                b,
                amount: maximum,
                minimum,
                deadline,
            }
            .abi_encode(),
            (Form::Curve, true) => IPool::targetSwapCall {
                a,
                b,
                maximum,
                desired,
                deadline,
            }
            .abi_encode(),
            (Form::Curve, false) => IPool::originSwapCall {
                a,
                b,
                spend: maximum,
                minimum,
                deadline,
            }
            .abi_encode(),
            (Form::Router, false) => IRouter::originSwapCall {
                quote: t.quote,
                a,
                b,
                amount: maximum,
                minimum,
                deadline,
            }
            .abi_encode(),
            (Form::Router, true) => return Bytes::new(),
        }
        .into()
    }
    pub(super) async fn included(&self, ctx: &Ctx, h: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            h,
            r,
            self.payer,
            self.terms.target,
            U256::ZERO,
            self.data(),
        )
        .await
    }
    pub(crate) fn measured(
        &self,
        logs: &[super::super::check::simulate::SimLog],
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
        payer: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (payer, input, output) != (self.payer, self.input, self.output)
            || input == output
            || [self.terms.pool, self.terms.target].contains(&payer)
        {
            return None;
        }
        let t = self.terms;
        let cash = |token, from, to| fill::moved_logs(logs, token, from, to);
        let spent = cash(input, Some(payer), None).checked_sub(cash(input, None, Some(payer)))?;
        let received =
            cash(output, None, Some(payer)).checked_sub(cash(output, Some(payer), None))?;
        if spent.is_zero()
            || spent > self.maximum
            || (t.desired.is_none() && spent != self.maximum)
            || received.is_zero()
            || received < t.desired.unwrap_or_default().max(self.minimum)
        {
            return None;
        }
        if let Form::Euler{v2,guarded,vaults}=t.form{
            super::euler_wallet::payment(self,logs,v2,guarded,vaults,spent,received)?;
        }else{
        let mut swaps = logs
            .iter()
            .filter(|l| l.address() == t.pool && l.topic0() == Some(&IPool::Trade::SIGNATURE_HASH));
        let event = IPool::Trade::decode_log_data(&swaps.next()?.inner.data).ok()?;
        let trader = if t.form == Form::Curve {
            payer
        } else {
            t.target
        };
        if swaps.next().is_some()
            || (event.trader, event.origin, event.target) != (trader, input, output)
        {
            return None;
        }
            if t.form!=Form::Module && ((event.originAmount,event.targetAmount)!=(spent,received)
                || cash(input,Some(payer),Some(t.target))!=spent || cash(output,Some(t.target),Some(payer))!=received){return None;}
        }
        if matches!(t.form,Form::Module | Form::Euler{guarded:true,..}) {
            let mut events = logs.iter().filter(|l| {
                l.address() == t.target && l.topic0() == Some(&IModule::Filled::SIGNATURE_HASH)
            });
            let e = IModule::Filled::decode_log_data(&events.next()?.inner.data).ok()?;
            if events.next().is_some()
                || (
                    e.payer,
                    e.recipient,
                    e.pool,
                    e.input,
                    e.output,
                    e.paid,
                    e.received,
                ) != (payer, payer, t.pool, input, output, spent, received)
            {
                return None;
            }
        }
        Some((spent, received))
    }
}

#[cfg(test)]
#[path = "stabull_wallet_tests.rs"]
mod tests;
