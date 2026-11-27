//! Рыночная котировка и фактический вход AMM после фиксированной комиссии за каждый pool.
//! https://documentation.ryze.pro/prompts/full-integration#execution-reality-rules
use super::super::math::{full_math, ten};
use super::{http, wire::Hop};
use alloy::primitives::{Address, U256};
use chrono::{DateTime, Utc};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};

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

pub struct Market<'a> {
    pub wallet: Address,
    pub input: Address,
    pub output: Address,
    pub decimals: (u8, u8),
    pub pools: &'a [(Address, Address, Address)],
    pub slippage_bps: u16,
    pub max_age_ms: u64,
}

pub struct Quoted {
    pub path: Vec<Hop>,
    pub gross: U256,
    pub net: U256,
    pub expected: U256,
    pub fee: U256,
    pub at: DateTime<Utc>,
}
struct Leg {
    path: Vec<Hop>,
    input: U256,
    output: U256,
    fee: U256,
    at: DateTime<Utc>,
}

fn units(s: &str) -> Option<U256> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    U256::from_str_radix(s, 10).ok()
}
pub fn input_fee(usd: U256, price: U256, decimals: u8, hops: usize) -> Option<U256> {
    if !(1..=3).contains(&hops) {
        return None;
    }
    full_math::mul_div_up(usd, ten::checked(u32::from(decimals))?, price)?
        .checked_mul(U256::from(hops))
}

impl Market<'_> {
    fn validate(&self, data: Value, usd: U256, now: DateTime<Utc>) -> Option<Leg> {
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
        let mut next = input;
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
        let fee = input_fee(
            usd,
            units(&prices.next()?.blended_price)?,
            self.decimals.0,
            path.len(),
        )?;
        if prices.next().is_some() {
            return None;
        }
        Some(Leg {
            path,
            input,
            output,
            fee,
            at: r.timestamp,
        })
    }

    pub async fn measure(
        &self,
        url: Url,
        limits: crate::gateway::http::Limits,
        usd_fee: U256,
        qty: U256,
        buy: bool,
    ) -> Option<Quoted> {
        let call = |exact_input: bool, n: U256| {
            let mut body = json!({"userAddress":self.wallet,"tokenIn":self.input,"tokenOut":self.output,
                "slippageTolerance":self.slippage_bps,"maxHops":self.pools.len().min(3)});
            body[if exact_input { "amountIn" } else { "amountOut" }] = json!(n.to_string());
            body
        };
        let raw = http::quote(url.clone(), &call(!buy, qty), limits).await?;
        let first = self.validate(raw, usd_fee, Utc::now())?;
        let (gross, net) = if buy && first.output == qty {
            (first.input.checked_add(first.fee)?, first.input)
        } else if !buy && first.input == qty {
            (qty, qty.checked_sub(first.fee)?)
        } else {
            return None;
        };
        if net.is_zero() {
            return None;
        }
        let raw = http::quote(url, &call(true, net), limits).await?;
        let paid = self.validate(raw, usd_fee, Utc::now())?;
        if paid.input != net
            || paid.fee != first.fee
            || paid.path != first.path
            || (buy && paid.output < qty)
        {
            return None;
        }
        Some(Quoted {
            path: paid.path,
            gross,
            net,
            expected: paid.output,
            fee: paid.fee,
            at: paid.at,
        })
    }
}
