//! Подписанное поручение Empire, квитанция исходной пары и измеренный прирост средств получателя.
use super::{fill, signed_call::Erc20Call};
use crate::venues::bsc_dex::pools::{abi_v2v3::IV2Pair, solidly_v3};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};

alloy::sol! { event Filled(address indexed pool,address indexed payer,address indexed recipient,address input,address output,
uint256 debit,uint256 credit,uint256 gross,uint256 received); }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    #[serde(default = "super::signed_call::legacy_ethereum_chain")]
    pub chain: u64,
    pub module: Address,
    pub pool: Address,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}

impl Proof {
    pub(super) fn data(&self) -> Bytes {
        solidly_v3::execution_data(
            self.pool,
            self.input,
            self.output,
            self.amount,
            self.minimum,
            self.deadline,
        )
    }
    pub(in crate::venues::bsc_dex) fn signed(&self) -> Erc20Call {
        Erc20Call {
            chain: self.chain,
            payer: self.payer,
            module: self.module,
            calldata: keccak256(self.data()),
        }
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        payer: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (payer, input, output) != (self.payer, self.input, self.output) || input == output {
            return None;
        }
        let mut fills = logs
            .iter()
            .filter(|l| l.address() == self.module && l.topic0() == Some(&Filled::SIGNATURE_HASH));
        let f = Filled::decode_log_data(&fills.next()?.inner.data).ok()?;
        if fills.next().is_some()
            || (f.pool, f.payer, f.recipient, f.input, f.output)
                != (self.pool, payer, payer, input, output)
            || f.debit != self.amount
            || f.credit.is_zero()
            || f.credit > f.debit
            || f.gross.is_zero()
            || f.received.is_zero()
            || f.received < self.minimum
        {
            return None;
        }
        let mut swaps = logs.iter().filter(|l| {
            l.address() == self.pool && l.topic0() == Some(&IV2Pair::Swap::SIGNATURE_HASH)
        });
        let s = IV2Pair::Swap::decode_log_data(&swaps.next()?.inner.data).ok()?;
        let (gross, other) = if input < output {
            (s.amount1Out, s.amount0Out)
        } else {
            (s.amount0Out, s.amount1Out)
        };
        if swaps.next().is_some()
            || s.sender != self.module
            || s.to != payer
            || gross != f.gross
            || !other.is_zero()
        {
            return None;
        }
        let cash = |token, from, to| fill::moved_logs(logs, token, from, to);
        let debit = cash(input, Some(payer), None).checked_sub(cash(input, None, Some(payer)))?;
        let from_pair = cash(output, Some(self.pool), Some(payer));
        // При перераспределении токена balanceOf меняется также между Transfer; Filled измерен внутри исходного вызова.
        (debit == f.debit
            && !from_pair.is_zero()
            && from_pair == cash(output, None, Some(payer))
            && cash(output, Some(payer), None).is_zero())
        .then_some((debit, f.received))
    }
}

#[cfg(test)]
#[path = "empire_wallet_tests.rs"]
mod tests;
