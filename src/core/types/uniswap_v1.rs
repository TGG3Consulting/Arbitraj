//! Physical ETH and ERC20 reserves of the registered Uniswap V1 exchanges.
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct V1Exchange {
    pub address: Address,
    pub token: Address,
    pub eth: U256,
    pub erc20: U256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniswapV1State {
    pub factory: Address,
    pub tokens: [Address; 2],
    /// None stands for the native side of a single exchange. Some(WETH) is a real ERC20 exchange.
    pub markets: [Option<V1Exchange>; 2],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    pub spent: U256,
    pub received: U256,
    /// Native wei exchanged between the two venues, or paid/received on the native side.
    pub eth: U256,
}

fn input(x: U256, y: U256, amount: U256) -> Option<U256> {
    if amount.is_zero() || x.is_zero() || y.is_zero() {
        return None;
    }
    let weighted = amount.checked_mul(U256::from(997))?;
    weighted
        .checked_mul(y)?
        .checked_div(x.checked_mul(U256::from(1000))?.checked_add(weighted)?)
}
fn output(x: U256, y: U256, want: U256) -> Option<U256> {
    if want.is_zero() || x.is_zero() || want >= y {
        return None;
    }
    x.checked_mul(want)?
        .checked_mul(U256::from(1000))?
        .checked_div(y.checked_sub(want)?.checked_mul(U256::from(997))?)?
        .checked_add(U256::ONE)
}

impl UniswapV1State {
    pub fn side(&self, t: Address) -> Option<usize> {
        self.tokens.iter().position(|a| *a == t)
    }
    pub fn dual(&self) -> bool {
        self.markets.iter().all(Option::is_some)
    }
    pub fn swap(&self, source: Address, exact_in: bool, amount: U256) -> Option<Fill> {
        let i = self.side(source)?;
        let a = self.markets[i].as_ref();
        let b = self.markets[1 - i].as_ref();
        if amount.is_zero() || (a.is_none() && b.is_none()) {
            return None;
        }
        let f = if exact_in {
            let eth = match a {
                Some(e) => input(e.erc20, e.eth, amount)?,
                None => amount,
            };
            let received = match b {
                Some(e) => input(e.eth, e.erc20, eth)?,
                None => eth,
            };
            Fill {
                spent: amount,
                received,
                eth,
            }
        } else {
            let eth = match b {
                Some(e) => output(e.eth, e.erc20, amount)?,
                None => amount,
            };
            let spent = match a {
                Some(e) => output(e.erc20, e.eth, eth)?,
                None => eth,
            };
            Fill {
                spent,
                received: amount,
                eth,
            }
        };
        (!f.received.is_zero() && !f.eth.is_zero()).then_some(f)
    }

    /// Two successive fee-free constant-product curves compose to one curve. Used only for band depth.
    pub fn band_reserves(&self, source: Address) -> Option<(U256, U256)> {
        use crate::venues::bsc_dex::math::full_math::mul_div;
        let i = self.side(source)?;
        match (&self.markets[i], &self.markets[1 - i]) {
            (Some(a), None) => Some((a.erc20, a.eth)),
            (None, Some(b)) => Some((b.eth, b.erc20)),
            (Some(a), Some(b)) => {
                let sum = a.eth.checked_add(b.eth)?;
                if sum.is_zero() {
                    return Some((U256::ZERO, U256::ZERO));
                }
                Some((mul_div(a.erc20, b.eth, sum)?, mul_div(b.erc20, a.eth, sum)?))
            }
            _ => None,
        }
    }

    /// Mid-price conversion for the depth selector, before pool fees and price impact.
    pub fn spot(&self, source: Address, amount: U256) -> Option<U256> {
        use crate::venues::bsc_dex::math::full_math::mul_div;
        let i = self.side(source)?;
        let eth = match &self.markets[i] {
            Some(e) => mul_div(amount, e.eth, e.erc20)?,
            None => amount,
        };
        match &self.markets[1 - i] {
            Some(e) => mul_div(eth, e.erc20, e.eth),
            None => Some(eth),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inverse_adds_one_even_at_exact_division() {
        assert_eq!(
            output(U256::from(997), U256::from(2), U256::ONE),
            Some(U256::from(1001))
        );
        assert!(output(U256::ONE, U256::ONE, U256::ONE).is_none());
    }
    #[test]
    fn both_fees_and_inverse_use_physical_native_intermediate() {
        let a = Address::repeat_byte(1);
        let b = Address::repeat_byte(2);
        let exchange = |token, raw, eth| {
            Some(V1Exchange {
                token,
                address: token,
                erc20: U256::from(raw),
                eth: U256::from(eth),
            })
        };
        let s = UniswapV1State {
            factory: Address::ZERO,
            tokens: [a, b],
            markets: [
                exchange(a, 210000u64, 1900000u64),
                exchange(b, 800000, 100000),
            ],
        };
        for t in [a, b] {
            let f = s.swap(t, true, U256::from(700)).unwrap();
            let inv = s.swap(t, false, f.received).unwrap();
            assert!(inv.spent <= f.spent);
            assert!(s.swap(t, true, inv.spent).unwrap().received >= f.received);
            assert!(f.received < s.spot(t, f.spent).unwrap());
        }
    }
    #[test]
    fn original_getters_agree_in_both_directions_including_empty_usdt_exchange() {
        use serde_json::Value;
        let evidence: Value = serde_json::from_str(include_str!(
            "../../../tests/data/first_run_dex_2026-11-26/uniswap_v1_ethereum_prices.json"
        ))
        .unwrap();
        let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let raw = |v: &Value| U256::from_str_radix(v.as_str().unwrap(), 10).unwrap();
        assert_eq!(evidence["block"], 26494988);
        assert_eq!(evidence["selector"]["requireCanonical"], true);
        let wrapped = address(&evidence["wrapped"]);
        let factory = address(&evidence["factory"]);
        let mut positive = 0;
        let mut rejected = 0;
        for sample in evidence["quotes"].as_array().unwrap() {
            let pair = [&sample["source"], &sample["target"]];
            let tokens = pair.map(|v| if v.is_null() { wrapped } else { address(v) });
            let markets = pair.map(|v| {
                v.as_str().map(|a| {
                    let m = &evidence["markets"][a];
                    V1Exchange {
                        address: address(&m["exchange"]),
                        token: address(v),
                        eth: raw(&m["eth"]),
                        erc20: raw(&m["erc20"]),
                    }
                })
            });
            let state = UniswapV1State {
                factory,
                tokens,
                markets,
            };
            let exact_in = sample["exact_in"].as_bool().unwrap();
            let size = raw(&sample["amount"]);
            let fill = state.swap(tokens[0], exact_in, size);
            let calls = sample["calls"].as_array().unwrap();
            assert_eq!(state.dual(), calls.len() == 2);
            assert_eq!(raw(&calls[0]["operand"]), size);
            if calls
                .iter()
                .any(|c| c["result"].is_null() || raw(&c["result"]).is_zero())
            {
                assert!(fill.is_none(), "{sample}");
                rejected += 1;
                continue;
            }
            let fill = fill.unwrap();
            let output = raw(&calls.last().unwrap()["result"]);
            assert_eq!(
                if exact_in {
                    (fill.spent, fill.received)
                } else {
                    (fill.received, fill.spent)
                },
                (size, output)
            );
            if state.dual() {
                assert_eq!(fill.eth, raw(&calls[0]["result"]));
                assert_eq!(fill.eth, raw(&calls[1]["operand"]));
            }
            for c in calls {
                assert_eq!(c["return_bytes"], 4096);
            }
            positive += 1;
        }
        assert_eq!((positive, rejected), (32, 16));
    }
}
