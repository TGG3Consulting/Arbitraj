//! Carry physical collection liquidity and the MasterVault cash across projected pair edges.
use super::route_calc::Leg;
use crate::core::types::{
    PoolRef, PoolState,
    bancor_v3::{BancorMarket, BancorState, Fill},
};
use alloy_primitives::{Address, U256};
use std::collections::HashMap;
pub(super) fn withdrawal(l: &Leg) -> Option<PoolRef> {
    let PoolState::BancorV3(s) = &l.entry.pool.state else {
        return None;
    };
    Some(PoolRef::Member {
        contract: s.vault,
        coin: s.physical[s.side(l.token_out)?],
    })
}
#[derive(Default)]
pub(super) struct Sequence {
    markets: HashMap<(Address, Address), BancorMarket>,
    vault: HashMap<(Address, Address), U256>,
}
impl Sequence {
    pub(super) fn current(&self, s: &BancorState) -> BancorState {
        let mut v = s.clone();
        for k in 0..2 {
            if let Some(m) = &s.markets[k]
                && let Some(n) = self.markets.get(&(s.network, m.token))
            {
                v.markets[k] = Some(n.clone());
            }
            if let Some(cash) = self.vault.get(&(s.vault, s.physical[k])) {
                v.vault_balances[k] = *cash;
            }
        }
        v
    }
    pub(super) fn advance(
        &mut self,
        s: &BancorState,
        input: Address,
        amount: U256,
    ) -> Option<Fill> {
        let (fill, next) = self.current(s).after(input, amount)?;
        for (k, m) in next.markets.into_iter().enumerate() {
            if let Some(m) = m {
                self.markets.insert((s.network, m.token), m);
            }
            self.vault
                .insert((s.vault, s.physical[k]), next.vault_balances[k]);
        }
        Some(fill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn addr(i: u8) -> Address {
        Address::repeat_byte(i)
    }
    fn state(a: u8, b: u8) -> BancorState {
        let bnt = addr(1);
        let tokens = [addr(a), addr(b)];
        let markets = tokens.map(|token| {
            (token != bnt).then_some(BancorMarket {
                token,
                collection: addr(12),
                base: U256::from(10000),
                bnt: U256::from(7000),
                staked: U256::from(13000),
                fee_ppm: 14000,
                network_fee_ppm: 1000000,
                enabled: true,
            })
        });
        BancorState {
            network: addr(10),
            info: addr(11),
            vault: addr(13),
            bnt,
            bnt_decimals: 18,
            wrapped: addr(14),
            tokens,
            physical: tokens,
            markets,
            vault_balances: tokens.map(|t| {
                if t == bnt {
                    U256::from(800)
                } else {
                    U256::from(20000)
                }
            }),
        }
    }
    #[test]
    fn independent_markets_cannot_withdraw_the_same_bnt_twice() {
        let first = state(1, 2);
        let second = state(1, 3);
        let qty = U256::from(1000);
        let mut series = Sequence::default();
        let debit = series.advance(&first, addr(2), qty).unwrap();
        assert!(second.swap(addr(3), true, qty, false).is_some());
        assert_eq!(
            series.current(&second).vault_balances[0],
            U256::from(800) - debit.received
        );
        assert!(series.advance(&second, addr(3), qty).is_none());
        assert!(series.advance(&second, addr(3), U256::from(80)).is_some());
    }
    #[test]
    fn double_trade_reprices_both_markets_and_never_withdraws_virtual_bnt() {
        let mut sequence = Sequence::default();
        let dual = state(2, 3);
        let a = state(1, 2);
        let b = state(1, 3);
        let qty = U256::from(600);
        let (_, next) = dual.after(addr(2), qty).unwrap();
        let fill = sequence.advance(&dual, addr(2), qty).unwrap();
        assert_eq!(sequence.current(&a).markets[1], next.markets[0]);
        assert_eq!(sequence.current(&b).markets[1], next.markets[1]);
        assert_eq!(sequence.current(&a).vault_balances[0], a.vault_balances[0]);
        assert_eq!(
            sequence.current(&b).vault_balances[1],
            b.vault_balances[1] - fill.received
        );
        let prior = a.swap(addr(2), true, qty, false).unwrap();
        let repeated = sequence.advance(&a, addr(2), qty).unwrap();
        assert!(repeated.received < prior.received);
    }
}
