//! Сверяет пару, разрядности, путь и непрерывность сумм ответа Router. Вход первого пула может быть меньше списания кошелька.
use super::{Hop, Market};
use alloy::primitives::{Address, U256};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Amount {
    token: Address,
    amount: String,
    decimals: u8,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    pool: Address,
    token_in: Address,
    token_out: Address,
    amount_in: String,
    amount_out: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Price {
    token: Address,
    blended_price: String,
}
#[derive(Deserialize)]
struct Reply {
    input: Amount,
    output: Amount,
    path: Vec<Address>,
    steps: Vec<Step>,
    prices: Vec<Price>,
    timestamp: DateTime<Utc>,
}

pub(super) struct Leg {
    pub(super) path: Vec<Hop>,
    pub(super) input: U256,
    pub(super) output: U256,
    pub(super) pool_input: U256,
    pub(super) price: Option<U256>,
    pub(super) at: DateTime<Utc>,
}

fn units(s: &str) -> Option<U256> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    U256::from_str_radix(s, 10).ok()
}
impl Market<'_> {
    pub(super) fn validate(&self, data: Value, now: DateTime<Utc>) -> Option<Leg> {
        let r: Reply = serde_json::from_value(data).ok()?;
        let age = now.signed_duration_since(r.timestamp).num_milliseconds();
        if self.wallet.is_zero()
            || self.input == self.output
            || self.slippage_bps >= 10000
            || !(1..=3).contains(&r.steps.len())
            || r.path.len() != r.steps.len() + 1
            || (r.input.token, r.output.token) != (self.input, self.output)
            || (r.input.decimals, r.output.decimals) != self.decimals
            || r.path.first() != Some(&self.input)
            || r.path.last() != Some(&self.output)
            || age < 0
            || age as u64 > self.max_age_ms
        {
            return None;
        }
        let input = units(&r.input.amount)?;
        let output = units(&r.output.amount)?;
        if input.is_zero() || output.is_zero() {
            return None;
        }
        let mut path: Vec<Hop> = Vec::new();
        let pool_input = units(&r.steps.first()?.amount_in)?;
        if pool_input.is_zero() || pool_input > input {
            return None;
        }
        let mut next = pool_input;
        for (s, pair) in r.steps.iter().zip(r.path.windows(2)) {
            if pair != [s.token_in, s.token_out]
                || !self.pools.iter().any(|&(p, a, b)| {
                    p == s.pool
                        && ((a, b) == (s.token_in, s.token_out)
                            || (a, b) == (s.token_out, s.token_in))
                })
                || path
                    .iter()
                    .any(|h| h.pool == s.pool || h.tokenIn == s.token_out)
                || units(&s.amount_in)? != next
            {
                return None;
            }
            next = units(&s.amount_out)?;
            if next.is_zero() {
                return None;
            }
            path.push(Hop {
                pool: s.pool,
                tokenIn: s.token_in,
                tokenOut: s.token_out,
            });
        }
        if next != output {
            return None;
        }
        let mut prices = r.prices.iter().filter(|p| p.token == self.input);
        let price = prices.next().and_then(|p| units(&p.blended_price));
        if prices.next().is_some() {
            return None;
        }
        Some(Leg {
            path,
            input,
            output,
            pool_input,
            price,
            at: r.timestamp,
        })
    }
}
