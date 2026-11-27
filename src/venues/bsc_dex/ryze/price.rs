//! Запрашивает котировку на полное списание кошелька. Для BUY exact-output задаёт начальный бюджет поиска.
//! https://documentation.ryze.pro/prompts/full-integration#quotes
use super::super::math::{full_math, ten};
use super::{http, wire::Hop};
use alloy::primitives::{Address, U256};
use chrono::{DateTime, Utc};
use reqwest::Url;
use serde_json::json;

#[path = "price_reply.rs"]
mod reply;
use reply::Leg;

pub struct Market<'a> {
    pub ctx: &'a super::super::ctx::Ctx,
    pub wallet: Address,
    pub input: Address,
    pub output: Address,
    pub decimals: (u8, u8),
    pub pools: &'a [(Address, Address, Address)],
    pub slippage_bps: u16,
    /// Реальный доступный вход, ограниченный кошельком и бюджетом конкретной покупки.
    pub buy_cap: U256,
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
impl Quoted {
    pub(super) fn covers(&self, target: U256) -> bool {
        self.expected >= target
    }
}

pub fn input_fee(usd: U256, price: U256, decimals: u8, hops: usize) -> Option<U256> {
    if !(1..=3).contains(&hops) {
        return None;
    }
    full_math::mul_div_up(usd, ten::checked(u32::from(decimals))?, price)?
        .checked_mul(U256::from(hops))
}

impl Market<'_> {
    async fn request(
        &self,
        url: &Url,
        limits: crate::gateway::http::Limits,
        exact_input: bool,
        n: U256,
    ) -> Option<Leg> {
        let mut body = json!({"userAddress":self.wallet,"tokenIn":self.input,"tokenOut":self.output,
            "slippageTolerance":self.slippage_bps,"maxHops":self.pools.len().min(3)});
        body[if exact_input { "amountIn" } else { "amountOut" }] = json!(n.to_string());
        let raw = http::quote(self.ctx, url, &body, limits).await?;
        let leg = self.validate(raw, Utc::now())?;
        let fixed = if exact_input { leg.input } else { leg.output };
        (fixed == n).then_some(leg)
    }

    /// Продажа измеряет весь запрошенный дебет один раз. Для покупки каждый проверяемый бюджет
    /// получает собственный exact-input ответ. В торговый выбор поступает только реально достигнутая цена Router.
    pub async fn measure(
        &self,
        url: Url,
        limits: crate::gateway::http::Limits,
        usd_fee: U256,
        qty: U256,
        buy: bool,
    ) -> Option<Quoted> {
        let first = self.request(&url, limits, !buy, qty).await?;
        if !buy {
            return first.exact_input();
        }
        // В ответе exact-output поле input может означать сумму первого пула. Оценка фиксированной
        // платы нужна только для поиска полной суммы. Источник условий исполнения — новый exact-input.
        let initial = if first.pool_input == first.input && !usd_fee.is_zero() {
            let fee = input_fee(usd_fee, first.price?, self.decimals.0, first.path.len())?;
            first.input.checked_add(fee)?
        } else {
            first.input
        };
        let cap = full_math::mul_div_up(
            initial,
            U256::from(10_000u32 + u32::from(self.slippage_bps)),
            U256::from(10_000),
        )?.min(self.buy_cap);
        if cap.is_zero() {
            return None;
        }
        let mut gross = initial.min(cap);
        for probe in 0..3 {
            let paid = self.request(&url, limits, true, gross).await?;
            if paid.path != first.path {
                return None;
            }
            let quoted = paid.exact_input()?;
            if quoted.covers(qty) {
                return Some(quoted);
            }
            if gross == cap {
                return None;
            }
            gross = if probe == 0 {
                full_math::mul_div_up(gross, qty, quoted.expected)?.min(cap)
            } else {
                cap
            };
        }
        None
    }
}

impl Leg {
    fn exact_input(self) -> Option<Quoted> {
        let fee = self.input.checked_sub(self.pool_input)?;
        Some(Quoted {
            path: self.path,
            gross: self.input,
            net: self.pool_input,
            expected: self.output,
            fee,
            at: self.at,
        })
    }
}
