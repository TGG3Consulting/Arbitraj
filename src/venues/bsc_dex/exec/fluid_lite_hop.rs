//! Atomic Fluid swapHop. Every physical swap uses the cash and pool state left by the preceding leg.
use super::{
    fill,
    fluid_lite_wallet::{Proof, Terms, abi::IWallet, scale},
};
use crate::{core::types::fluid_dex_lite::Market, venues::bsc_dex::quote::v2_wallet_sim::Order};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Route {
    pub markets: Vec<Market>,
    pub path: Vec<Address>,
}

impl Route {
    pub fn physical(&self, wrapped: Address) -> Option<Vec<Address>> {
        if self.markets.len() < 2
            || self.path.len() != self.markets.len() + 1
            || self.path.first() == self.path.last()
        {
            return None;
        }
        let mut physical = Vec::with_capacity(self.path.len());
        for (market, edge) in self.markets.iter().zip(self.path.windows(2)) {
            let [a, b] = market.tokens(wrapped)?;
            let (from, to) = match (edge[0], edge[1]) {
                (x, y) if (x, y) == (a, b) => (market.token0, market.token1),
                (x, y) if (x, y) == (b, a) => (market.token1, market.token0),
                _ => return None,
            };
            if let Some(prev) = physical.last() {
                if *prev != from {
                    return None;
                }
            } else {
                physical.push(from);
            }
            physical.push(to);
        }
        Some(physical)
    }
    fn ids(&self) -> Option<Vec<B256>> {
        self.markets.iter().map(|m| m.id()).collect()
    }
    pub(crate) fn proof(
        &self,
        terms: Terms,
        order: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        let mut p = terms.proof(order, minimum, deadline);
        p.route = Some(self.clone());
        p
    }
    fn valid(&self, p: &Proof) -> Option<()> {
        self.physical(p.terms.wrapped)?;
        (self.markets.first() == Some(&p.terms.market)
            && self.path.first() == Some(&p.input)
            && self.path.last() == Some(&p.output)
            && p.terms.desired.is_none()
            && !p.maximum.is_zero()
            && !p.minimum.is_zero()
            && !p.deadline.is_zero())
        .then_some(())
    }
    pub(super) fn data(&self, p: &Proof) -> Option<Bytes> {
        self.valid(p)?;
        Some(
            IWallet::executeHopCall {
                markets: self.ids()?,
                path: self.path.clone(),
                amountIn: p.maximum,
                minimum: p.minimum,
                deadline: p.deadline,
            }
            .abi_encode()
            .into(),
        )
    }

    /// Original LogSwap normalizes both sides to nine decimals; swapHop sends each raw result into the next leg.
    pub(super) fn amounts(&self, p: &Proof, logs: &[Log]) -> Option<(U256, U256)> {
        self.valid(p)?;
        let physical = self.physical(p.terms.wrapped)?;
        let mut fills = logs.iter().filter(|l| {
            l.address() == p.terms.target && l.topic0() == Some(&IWallet::HopFilled::SIGNATURE_HASH)
        });
        let ev = IWallet::HopFilled::decode_log_data(&fills.next()?.inner.data).ok()?;
        let route = keccak256((self.ids()?, self.path.clone()).abi_encode_params());
        if fills.next().is_some()
            || (ev.route, ev.payer, ev.recipient, ev.input, ev.output)
                != (route, p.payer, p.payer, p.input, p.output)
            || ev.spent != p.maximum
            || ev.received < p.minimum
        {
            return None;
        }
        self.flows(p, logs)?;
        let endpoint = |asset: Address| {
            if asset == Address::repeat_byte(0xee) {
                p.terms.target
            } else {
                p.terms.dex
            }
        };
        let moved = |asset, from, to| fill::moved_logs(logs, asset, from, to);
        let debit =
            moved(p.input, Some(p.payer), None).checked_sub(moved(p.input, None, Some(p.payer)))?;
        let received = moved(p.output, None, Some(p.payer)).checked_sub(moved(
            p.output,
            Some(p.payer),
            None,
        ))?;
        ((debit, received) == (ev.spent, ev.received)
            && !moved(p.input, Some(p.payer), Some(endpoint(physical[0]))).is_zero()
            && !moved(p.output, Some(endpoint(*physical.last()?)), Some(p.payer)).is_zero())
        .then_some((debit, received))
    }
    pub(crate) fn flows(&self, p: &Proof, logs: &[Log]) -> Option<Vec<(U256, U256)>> {
        let mut swaps = logs.iter().filter(|l| {
            l.address() == p.terms.dex && l.topic0() == Some(&IWallet::LogSwap::SIGNATURE_HASH)
        });
        let physical = self.physical(p.terms.wrapped)?;
        let mut amount = p.maximum;
        let mut flows = Vec::with_capacity(self.markets.len());
        let mask: U256 = (U256::ONE << 60) - U256::ONE;
        for (i, market) in self.markets.iter().enumerate() {
            let swap = IWallet::LogSwap::decode_log_data(&swaps.next()?.inner.data).ok()?;
            let forward = physical[i] == market.token0;
            if swap.swapData.bit(64) != forward
                || (swap.swapData & U256::from(u64::MAX)) != U256::from_be_slice(&market.id()?[..8])
            {
                return None;
            }
            let decimals =
                |shift: usize| ((swap.dexVariables >> shift) & U256::from(31)).to::<u8>();
            let (din, dout) = if forward {
                (decimals(126), decimals(131))
            } else {
                (decimals(131), decimals(126))
            };
            if scale(amount, din, 9)? != ((swap.swapData >> 65) & mask) {
                return None;
            }
            let debit = amount;
            amount = scale((swap.swapData >> 125) & mask, 9, dout)?;
            if amount.is_zero() {
                return None;
            }
            flows.push((debit, amount));
        }
        if swaps.next().is_some() {
            return None;
        }
        Some(flows)
    }
}
