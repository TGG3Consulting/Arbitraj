//! The original LLAMMA event indexes its recipient. This proof also binds the signed payer,
//! returned change and the two real ERC20 transfers. Sources: contracts/LlammaWallet.sol,
//! https://docs.curve.finance/developer/integration/llamma/ and ERC-20 Transfer.
use super::{fill, signed_call};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        check::simulate::SimLog,
        ctx::Ctx,
        pools::{llamma::ILlama, solidly_classic_factory},
        quote::v2_wallet_sim::Order,
        send::track::Receipt,
    },
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
pub(in crate::venues::bsc_dex) mod abi {
    alloy::sol! {
        interface IModule{
            struct Order{uint256 index;address pool;bytes32 poolHash;address input;address output;uint256 maximum;uint256 minimum;uint256 deadline;bool byTarget;}
            function execute(Order order) external returns(uint256 spent,uint256 received);
            function factory() external view returns(address);
            function factoryHash() external view returns(bytes32);
            function tupleRegistry() external view returns(bool);
            event Paid(address indexed payer,address indexed pool,address indexed input,address output,uint256 spent,uint256 received);
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub index: u64,
    pub pool: Address,
    pub pool_hash: B256,
    pub target: Address,
    pub module_hash: B256,
    pub zero: Address,
    pub desired: Option<U256>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
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
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            terms: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            maximum: o.amount,
            minimum: self.desired.unwrap_or(minimum),
            deadline,
        }
    }
}
impl Proof {
    pub fn data(&self) -> Bytes {
        abi::IModule::executeCall {
            order: abi::IModule::Order {
                index: U256::from(self.terms.index),
                pool: self.terms.pool,
                poolHash: self.terms.pool_hash,
                input: self.input,
                output: self.output,
                maximum: self.maximum,
                minimum: self.minimum,
                deadline: self.deadline,
                byTarget: self.terms.desired.is_some(),
            },
        }
        .abi_encode()
        .into()
    }
    pub(super) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        receipt: &Receipt,
    ) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            receipt,
            self.payer,
            self.terms.target,
            U256::ZERO,
            self.data(),
        )
        .await?;
        let block = receipt.block_hash.ok_or_else(|| {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "LLAMMA: отсутствует блок квитанции",
            )
        })?;
        let at = serde_json::json!({"blockHash":block,"requireCanonical":true});
        solidly_classic_factory::plain(ctx, self.terms.target, self.terms.module_hash, &at).await?;
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx, hash, receipt).await
    }
    pub fn measured(&self, logs: &[SimLog]) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(logs)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub fn amounts(
        &self,
        logs: &[Log],
        payer: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        fn one<E: SolEvent>(logs: &[Log], at: Address) -> Option<E> {
            let mut hit = logs
                .iter()
                .filter(|v| v.address() == at && v.topic0() == Some(&E::SIGNATURE_HASH));
            let v = E::decode_log_data(&hit.next()?.inner.data).ok()?;
            hit.next().is_none().then_some(v)
        }
        let t = self.terms;
        if (payer, input, output) != (self.payer, self.input, self.output)
            || self.minimum.is_zero()
            || input == output
            || !(t.zero == input || t.zero == output)
        {
            return None;
        }
        let paid = one::<abi::IModule::Paid>(logs, t.target)?;
        let swap = one::<ILlama::TokenExchange>(logs, t.pool)?;
        let i = U256::from(u8::from(input != t.zero));
        if (paid.payer, paid.pool, paid.input, paid.output) != (payer, t.pool, input, output)
            || paid.spent.is_zero()
            || paid.spent > self.maximum
            || paid.received < self.minimum
            || swap.recipient != payer
            || swap.sold_id != i
            || swap.bought_id != U256::ONE - i
            || swap.tokens_sold != paid.spent
            || swap.tokens_bought.is_zero()
            || t.desired.is_some_and(|want| want != swap.tokens_bought)
        {
            return None;
        }
        let flow = |token, src, to| fill::moved_logs(logs, token, src, to);
        let sent = flow(input, Some(payer), Some(t.target));
        let refund = flow(input, Some(t.target), Some(payer));
        (sent == self.maximum
            && sent.checked_sub(refund)? == paid.spent
            && flow(input, Some(t.target), Some(t.pool)) == paid.spent
            && flow(output, Some(t.pool), Some(payer)) == paid.received
            && flow(output, None, Some(payer)).checked_sub(flow(output, Some(payer), None))?
                == paid.received
            && flow(input, Some(payer), None).checked_sub(flow(input, None, Some(payer)))?
                == paid.spent)
            .then_some((paid.spent, paid.received))
    }
}

pub(in crate::venues::bsc_dex) fn measured_v5(
    o: &Order,
    pool: Address,
    zero: Address,
    logs: &[SimLog],
) -> Option<(U256, U256)> {
    let logs = signed_call::logs(logs)?;
    let mut swaps = logs.iter().filter(|v| {
        v.address() == pool && v.topic0() == Some(&ILlama::TokenExchange::SIGNATURE_HASH)
    });
    let swap = ILlama::TokenExchange::decode_log_data(&swaps.next()?.inner.data).ok()?;
    let i = U256::from(u8::from(o.input != zero));
    if swaps.next().is_some()
        || swap.recipient != o.target
        || swap.sold_id != i
        || swap.bought_id != U256::ONE - i
        || swap.tokens_sold.is_zero()
        || swap.tokens_bought.is_zero()
    {
        return None;
    }
    let flow = |token, from, to| fill::moved_logs(&logs, token, Some(from), Some(to));
    let spent = flow(o.input, o.payer, o.target).checked_sub(flow(o.input, o.target, o.payer))?;
    let out = flow(o.output, o.target, o.payer);
    (spent == swap.tokens_sold
        && flow(o.input, o.target, pool) == spent
        && flow(o.output, pool, o.target) == swap.tokens_bought
        && out >= o.minimum)
        .then_some((spent, out))
}

#[cfg(test)]
#[path = "llamma_wallet_tests.rs"]
mod tests;
