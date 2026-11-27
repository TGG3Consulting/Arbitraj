//! Bancor PoolCollection v12, pinned source 97d7905b, _processTrade and _processNetworkFee.
use crate::venues::bsc_dex::math::full_math::mul_div;
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

const PPM: u32 = 1_000_000;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BancorMarket {
    pub token: Address,
    pub collection: Address,
    pub base: U256,
    pub bnt: U256,
    pub staked: U256,
    pub fee_ppm: u32,
    pub network_fee_ppm: u32,
    pub enabled: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BancorState {
    pub network: Address,
    pub info: Address,
    pub vault: Address,
    pub bnt: Address,
    pub bnt_decimals: u8,
    pub wrapped: Address,
    pub tokens: [Address; 2],
    pub physical: [Address; 2],
    pub markets: [Option<BancorMarket>; 2],
    pub vault_balances: [U256; 2],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub spent: U256,
    pub received: U256,
    pub bridge_bnt: U256,
    pub fees: [Option<(Address, U256)>; 2],
}
struct Hop {
    paid: U256,
    received: U256,
    fee: U256,
    next: [U256; 3],
}

impl BancorMarket {
    fn trade(&self, from_bnt: bool, amount: U256, exact_in: bool, exempt: bool) -> Option<Hop> {
        let (x, y) = if from_bnt {
            (self.bnt, self.base)
        } else {
            (self.base, self.bnt)
        };
        if !self.enabled
            || x.is_zero()
            || y.is_zero()
            || amount.is_zero()
            || self.fee_ppm > PPM
            || self.network_fee_ppm > PPM
        {
            return None;
        }
        let ppm = U256::from(PPM);
        let rate = U256::from(if exempt { 0 } else { self.fee_ppm });
        let (paid, received, fee) = if exact_in {
            let gross = mul_div(y, amount, x.checked_add(amount)?)?;
            let fee = mul_div(gross, rate, ppm)?;
            (amount, gross.checked_sub(fee)?, fee)
        } else {
            // tradeByTargetAmount intentionally floors BOTH divisions in the deployed contract.
            let fee = mul_div(amount, rate, ppm.checked_sub(rate)?)?;
            let gross = amount.checked_add(fee)?;
            (mul_div(x, gross, y.checked_sub(gross)?)?, amount, fee)
        };
        if paid.is_zero() || received.is_zero() {
            return None;
        }
        let mut sx = x.checked_add(paid)?;
        let mut ty = y.checked_sub(received)?;
        let mut staked = if from_bnt {
            self.staked.checked_add(fee)?
        } else {
            self.staked
        };
        if self.network_fee_ppm != 0 {
            let cut = mul_div(fee, U256::from(self.network_fee_ppm), ppm)?;
            let net = ty.checked_sub(cut)?;
            if from_bnt {
                if net.is_zero() {
                    return None;
                }
                sx = sx.checked_sub(mul_div(sx, cut, ty)?)?;
                staked = staked.checked_sub(cut)?;
            } else {
                ty = net;
            }
        }
        (sx.bit_len() <= 128 && ty.bit_len() <= 128).then_some(Hop {
            paid,
            received,
            fee,
            next: if from_bnt {
                [ty, sx, staked]
            } else {
                [sx, ty, staked]
            },
        })
    }
}
impl BancorState {
    pub fn side(&self, t: Address) -> Option<usize> {
        self.tokens.iter().position(|v| *v == t)
    }
    pub fn swap(
        &self,
        input: Address,
        exact_in: bool,
        amount: U256,
        ignore_fees: bool,
    ) -> Option<Fill> {
        self.trade(input, exact_in, amount, ignore_fees, |_, _| {})
    }
    /// Minimal input for a by-source wire. Bancor by-target applies a different rounding rule.
    pub fn input_for_forward(&self, input: Address, desired: U256, exempt: bool) -> Option<Fill> {
        use crate::venues::bsc_dex::math::full_math::mul_div_up;
        let i = self.side(input)?;
        let ppm = U256::from(PPM);
        let mut n = desired;
        for j in [1 - i, i] {
            if let Some(m) = &self.markets[j] {
                let (x, y) = if j == i {
                    (m.base, m.bnt)
                } else {
                    (m.bnt, m.base)
                };
                let rate = U256::from(if exempt { 0 } else { m.fee_ppm });
                let gross = mul_div(n.checked_sub(U256::ONE)?, ppm, ppm.checked_sub(rate)?)?
                    .checked_add(U256::ONE)?;
                n = mul_div_up(x, gross, y.checked_sub(gross)?)?;
            }
        }
        self.swap(input, true, n, exempt)
            .filter(|f| f.received >= desired)
    }
    pub fn after(&self, input: Address, amount: U256) -> Option<(Fill, Self)> {
        let mut state = self.clone();
        let fill = self.trade(input, true, amount, false, |i, [base, bnt, staked]| {
            if let Some(m) = &mut state.markets[i] {
                m.base = base;
                m.bnt = bnt;
                m.staked = staked;
            }
        })?;
        let i = self.side(input)?;
        // The Vault can receive less than the nominal ERC20 input. Credit only proven native deposits.
        if self.physical[i] != self.tokens[i] {
            state.vault_balances[i] = state.vault_balances[i].checked_add(fill.spent)?;
        }
        state.vault_balances[1 - i] = state.vault_balances[1 - i].checked_sub(fill.received)?;
        Some((fill, state))
    }
    fn trade(
        &self,
        input: Address,
        exact_in: bool,
        amount: U256,
        ignore_fees: bool,
        mut apply: impl FnMut(usize, [U256; 3]),
    ) -> Option<Fill> {
        let i = self.side(input)?;
        let j = 1 - i;
        let mut n = amount;
        let mut bridge = amount;
        let mut fees = [None; 2];
        if self.markets.iter().all(Option::is_none) {
            return None;
        }
        for k in if exact_in { [i, j] } else { [j, i] } {
            if let Some(m) = &self.markets[k] {
                let from_bnt = k == j;
                let hop = m.trade(from_bnt, n, exact_in, ignore_fees)?;
                fees[usize::from(from_bnt)] =
                    Some((if from_bnt { m.token } else { self.bnt }, hop.fee));
                apply(k, hop.next);
                bridge = if from_bnt { hop.paid } else { hop.received };
                n = if exact_in { hop.received } else { hop.paid };
            }
        }
        let (spent, received) = if exact_in { (amount, n) } else { (n, amount) };
        (received <= self.vault_balances[j]).then_some(Fill {
            spent,
            received,
            bridge_bnt: bridge,
            fees,
        })
    }
    fn curves(&self, input: Address) -> Option<[(U256, U256); 2]> {
        let i = self.side(input)?;
        if self
            .markets
            .iter()
            .flatten()
            .any(|m| !m.enabled || m.base.is_zero() || m.bnt.is_zero())
        {
            return None;
        }
        Some([
            self.markets[i]
                .as_ref()
                .map_or((U256::ONE, U256::ONE), |m| (m.base, m.bnt)),
            self.markets[1 - i]
                .as_ref()
                .map_or((U256::ONE, U256::ONE), |m| (m.bnt, m.base)),
        ])
    }
    pub fn spot(&self, input: Address, amount: U256) -> Option<U256> {
        let [(x, y), (z, w)] = self.curves(input)?;
        // Trading liquidity is uint128; this product fits in 512 bits.
        type W = alloy_primitives::U512;
        let n = (W::from(amount) * W::from(y) * W::from(w)).checked_div(W::from(x) * W::from(z))?;
        (n.bit_len() <= 256).then(|| U256::from(n))
    }
    pub fn band_reserves(&self, input: Address) -> Option<(U256, U256)> {
        let i = self.side(input)?;
        let [(x, y), (z, w)] = self.curves(input)?;
        match (self.markets[i].is_some(), self.markets[1 - i].is_some()) {
            (true, true) => {
                let sum = y.checked_add(z)?;
                Some((mul_div(x, z, sum)?, mul_div(w, y, sum)?))
            }
            (true, false) => Some((x, y)),
            (false, true) => Some((z, w)),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "bancor_v3_tests.rs"]
mod tests;
