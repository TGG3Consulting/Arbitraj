//! ABI официальной заявки OpenOcean V4 и исходного платежа владельца. До подписи проверяются оба получателя.
use super::{
    super::exec::{carbon_binding::Code, fill},
    wire::Plan,
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};

#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! {
        struct Description{address src;address dst;address from;address receiver;uint256 amount;uint256 minimum;uint256 guaranteed;uint256 flags;address referrer;bytes permit;}
        struct Call{uint256 target;uint256 gasLimit;uint256 value;bytes data;}
        function swap(address executor,Description trade,Call[] steps) returns(uint256);
        struct Order{address input;address output;uint256 amount;uint256 minimum;uint256 deadline;uint16 feeBps;bool throughVenue;}
        function trade(Order o,bytes payload) returns(uint256);
        event Swapped(address indexed sender,address indexed srcToken,address indexed dstToken,address dstReceiver,uint256 amount,uint256 spentAmount,uint256 returnAmount,uint256 minReturnAmount,uint256 guaranteedAmount,address referrer);
        event Settled(address indexed payer,address indexed input,address indexed output,bytes32 orderHash,uint256 spent,uint256 received,uint256 protocolFee,bool throughVenue);
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Ocean {
    pub proxy: Address,
    pub implementation: Code,
    pub executor: Address,
    pub nested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) funding: Option<Box<super::ocean_funding::Funding>>,
    pub payload: Bytes,
}
impl Ocean {
    fn request(&self, p: &Plan) -> Option<(abi::swapCall, abi::Order)> {
        let caller = if self.nested { p.router } else { p.wallet? };
        let n = p.amount.checked_sub(p.fee()?)?;
        if n.is_zero() || !self.nested && p.fee_bps != 0 {
            return None;
        }
        let req = abi::swapCall::abi_decode_validate(&self.payload).ok()?;
        let d = &req.trade;
        if req.executor != self.executor
            || req.steps.is_empty()
            || self.payload.as_ref() != req.abi_encode()
            || (d.src, d.dst, d.receiver, d.amount) != (p.input, p.output, caller, n)
            || d.from != self.executor
                && !self.funding.as_ref().is_some_and(|f| {
                    let c = &req.steps[0];
                    f.payment(
                        d.from,
                        [d.src, d.dst],
                        self.executor,
                        c.target,
                        c.value,
                        &c.data,
                    )
                })
            || d.minimum.is_zero()
            || d.flags != U256::from(2)
            || !d.referrer.is_zero()
            || !d.permit.is_empty()
        {
            return None;
        }
        Some((
            req,
            abi::Order {
                input: p.input,
                output: p.output,
                amount: p.amount,
                minimum: p.minimum,
                deadline: U256::from(p.deadline),
                feeBps: p.fee_bps,
                throughVenue: self.nested,
            },
        ))
    }
    pub(super) fn prepaid(&self, p: &Plan) -> Option<bool> {
        Some(self.request(p)?.0.trade.from != self.executor)
    }
    pub(super) fn data(&self, p: &Plan) -> Option<Bytes> {
        let (_, o) = self.request(p)?;
        Some(
            abi::tradeCall {
                o,
                payload: self.payload.clone(),
            }
            .abi_encode()
            .into(),
        )
    }
    pub(super) fn amounts(&self, p: &Plan, owner: Address, logs: &[Log]) -> Option<(U256, U256)> {
        let (req, o) = self.request(p)?;
        let module = p.wallet?;
        let wallet_event = super::wire::one(logs, module, abi::Settled::SIGNATURE_HASH)?;
        let x = abi::Settled::decode_log_data(&wallet_event.inner.data).ok()?;
        let spent = fill::moved_logs(logs, p.input, Some(owner), None)
            .checked_sub(fill::moved_logs(logs, p.input, None, Some(owner)))?;
        let received = fill::moved_logs(logs, p.output, None, Some(owner))
            .checked_sub(fill::moved_logs(logs, p.output, Some(owner), None))?;
        if spent != p.amount
            || received < p.minimum
            || (
                x.payer,
                x.input,
                x.output,
                x.spent,
                x.received,
                x.protocolFee,
                x.throughVenue,
                x.orderHash,
            ) != (
                owner,
                p.input,
                p.output,
                spent,
                received,
                p.fee()?,
                self.nested,
                keccak256((o, keccak256(&self.payload)).abi_encode()),
            )
        {
            return None;
        }
        let y = abi::Swapped::decode_log_data(
            &super::wire::one(logs, self.proxy, abi::Swapped::SIGNATURE_HASH)?
                .inner
                .data,
        )
        .ok()?;
        let caller = if self.nested { p.router } else { module };
        let d = req.trade;
        if (
            y.sender,
            y.srcToken,
            y.dstToken,
            y.dstReceiver,
            y.amount,
            y.spentAmount,
            y.minReturnAmount,
            y.guaranteedAmount,
            y.referrer,
        ) != (
            caller,
            p.input,
            p.output,
            caller,
            d.amount,
            d.amount,
            d.minimum,
            d.guaranteed,
            d.referrer,
        ) || y.returnAmount < received
        {
            return None;
        }
        if d.from != self.executor
            && !self
                .funding
                .as_ref()?
                .paid(p, owner, caller, self.executor, d.amount, logs)
        {
            return None;
        }
        if self.nested && !super::wire::venue_event(p, module, logs) {
            return None;
        }
        Some((spent, received))
    }
}
