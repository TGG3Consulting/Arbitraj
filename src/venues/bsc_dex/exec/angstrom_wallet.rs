//! Точное поручение Angstrom и восстановление фактических переводов по подписанным данным.
use super::{fill, signed_call};
use crate::venues::bsc_dex::{pools::abi_v4::IUniswapV4, quote::v2_wallet_sim::Order};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use alloy_primitives::{
    Address, Bytes, U256,
    aliases::{I24, U24},
    keccak256,
};
use serde::{Deserialize, Serialize};

#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! {
        interface IWallet {
            struct Key {address currency0;address currency1;uint24 fee;int24 tickSpacing;address hooks;}
            function execute(Key key,bool zeroForOne,uint128 amount,uint128 minimum,uint64 height,bytes attestation,uint256 deadline) external returns(uint256);
            function adapter() external view returns(address);
            function hook() external view returns(address);
            function manager() external view returns(address);
            event Paid(bytes32 indexed poolId,address indexed payer,address indexed input,address output,uint128 spent,uint256 received,uint64 height);
        }
    }
}
pub(super) use abi::IWallet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub chain: u64,
    pub module: Address,
    pub adapter: Address,
    pub manager: Address,
    pub hook: Address,
    pub tokens: [Address; 2],
    pub fee: U24,
    pub spacing: I24,
    pub height: u64,
    pub attestation: Bytes,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Terms {
    pub(in crate::venues::bsc_dex) fn proof(
        &self,
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            terms: self.clone(),
            payer: o.payer,
            input: o.input,
            output: o.output,
            amount: o.amount,
            minimum,
            deadline,
        }
    }
    fn key(&self) -> IWallet::Key {
        IWallet::Key {
            currency0: self.tokens[0],
            currency1: self.tokens[1],
            fee: self.fee,
            tickSpacing: self.spacing,
            hooks: self.hook,
        }
    }
}
impl Proof {
    pub(crate) fn data(&self) -> Bytes {
        let (Ok(amount), Ok(minimum)) = (u128::try_from(self.amount), u128::try_from(self.minimum))
        else {
            return Bytes::new();
        };
        let t = &self.terms;
        IWallet::executeCall {
            key: t.key(),
            zeroForOne: self.input == t.tokens[0],
            amount,
            minimum,
            height: t.height,
            attestation: t.attestation.clone(),
            deadline: self.deadline,
        }
        .abi_encode()
        .into()
    }
    pub(super) fn signed(&self) -> signed_call::Erc20Call {
        signed_call::Erc20Call {
            chain: self.terms.chain,
            payer: self.payer,
            module: self.terms.module,
            calldata: keccak256(self.data()),
        }
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
        payer: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let t = &self.terms;
        if (payer, input, output) != (self.payer, self.input, self.output)
            || input == output
            || !(t.tokens.contains(&input) && t.tokens.contains(&output))
        {
            return None;
        }
        let id = keccak256(t.key().abi_encode());
        let mut payments = logs.iter().filter(|l| {
            l.address() == t.module && l.topic0() == Some(&IWallet::Paid::SIGNATURE_HASH)
        });
        let p = IWallet::Paid::decode_log_data(&payments.next()?.inner.data).ok()?;
        if payments.next().is_some()
            || (p.poolId, p.payer, p.input, p.output, p.height)
                != (id, payer, input, output, t.height)
            || U256::from(p.spent) != self.amount
            || p.received < self.minimum
            || p.received.is_zero()
        {
            return None;
        }
        let mut swaps = logs.iter().filter(|l| {
            l.address() == t.manager
                && l.topic0() == Some(&IUniswapV4::Swap::SIGNATURE_HASH)
                && l.topics().get(1) == Some(&id)
        });
        let s = IUniswapV4::Swap::decode_log_data(&swaps.next()?.inner.data).ok()?;
        if swaps.next().is_some() || s.sender != t.adapter {
            return None;
        }
        let cash = |token, from, to| fill::moved_logs(logs, token, from, to);
        let spent = cash(input, Some(payer), None).checked_sub(cash(input, None, Some(payer)))?;
        let received =
            cash(output, None, Some(payer)).checked_sub(cash(output, Some(payer), None))?;
        (spent == self.amount
            && received == p.received
            && cash(input, Some(payer), Some(t.module)) == spent
            && cash(input, Some(t.module), Some(t.manager)) == spent
            && cash(output, Some(t.manager), Some(payer)) == received)
            .then_some((spent, received))
    }
}

#[cfg(test)]
#[path = "angstrom_wallet_tests.rs"]
mod tests;
