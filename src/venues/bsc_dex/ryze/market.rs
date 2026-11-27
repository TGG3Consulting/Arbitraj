//! Измеряет Ryze на конкретном кошельке. Физические активы, разрешение списания, состояние роутера, nonce и комиссия проверяются на общем head.
use super::super::{
    math::full_math,
    node::{abi::IERC20, call::canonical},
    num,
    parts::Parts,
    quote::white_state::{self, Head},
    wallet::signer,
};
use super::{
    deployment::Config,
    http,
    price::{self, Quoted},
    wire::{self, SwapIntent},
};
use crate::core::types::Side;
use alloy::primitives::{Address, U256};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

#[derive(Clone)]
pub struct Ticket {
    pub config: Config,
    pub usd_fee: U256,
    pub order: SwapIntent,
    pub at: DateTime<Utc>,
    pub decimals: (u8, u8),
    pub qty: Decimal,
    pub side: Side,
    pub block: u64,
}
impl Ticket {
    pub fn valid(&self, now: DateTime<Utc>) -> bool {
        let age = now.signed_duration_since(self.at).num_milliseconds();
        age >= 0
            && (age as u64) <= self.config.max_age_ms
            && U256::from(now.timestamp().max(0) as u64) < self.order.deadline
    }
}
pub struct Snapshot {
    pub h: Head,
    pub config: Config,
    pub markets: Vec<(Address, Address, Address)>,
    pub fee: U256,
    pub wallet: Address,
    pub cash: Address,
}
impl Snapshot {
    pub async fn read(p: &Parts, config: Config) -> Option<Self> {
        let wallet = signer::address(&p.ctx).ok()?;
        let cash = p.tokens.usdt(&p.ctx).ok()?;
        let h = white_state::head(&p.ctx, "latest").await.ok()?;
        let (markets, fee) = config.verify(&p.ctx, h).await.ok()?;
        Some(Self {
            h,
            config,
            markets,
            fee,
            wallet,
            cash,
        })
    }
    pub async fn measure(
        &self,
        p: &Parts,
        asset: Address,
        side: Side,
        qty: Decimal,
    ) -> Option<(Ticket, Decimal)> {
        let c = &self.config;
        let buy = side == Side::Buy;
        let (input, output) = if buy {
            (self.cash, asset)
        } else {
            (asset, self.cash)
        };
        if !self
            .markets
            .iter()
            .any(|&(_, a, b)| a == asset || b == asset)
            || input == output
        {
            return None;
        }
        let at = self.h.selector();
        let ctx = &p.ctx;
        let (id, od, nonce, balance, allow) = tokio::try_join!(
            canonical(ctx, &at, input, IERC20::decimalsCall {}),
            canonical(ctx, &at, output, IERC20::decimalsCall {}),
            canonical(
                ctx,
                &at,
                c.router,
                wire::swapNoncesCall { user: self.wallet }
            ),
            canonical(
                ctx,
                &at,
                input,
                IERC20::balanceOfCall { owner: self.wallet }
            ),
            canonical(
                ctx,
                &at,
                input,
                IERC20::allowanceCall {
                    owner: self.wallet,
                    spender: c.router
                }
            )
        )
        .ok()?;
        let decs = if buy { (od, id) } else { (id, od) };
        let units = num::to_units(qty, decs.0)?;
        if units.is_zero() || num::to_decimal(units, decs.0) != Some(qty) {
            return None;
        }
        let m = price::Market {
            wallet: self.wallet,
            input,
            output,
            decimals: (id, od),
            pools: &self.markets,
            slippage_bps: c.slippage,
            max_age_ms: c.max_age_ms,
        };
        let Quoted {
            path,
            gross,
            expected,
            at: quoted_at,
            ..
        } = m
            .measure(
                http::endpoint(&c.api, "quote")?,
                ctx.limits().ok()?,
                self.fee,
                units,
                buy,
            )
            .await?;
        if balance < gross || allow < gross {
            return None;
        }
        if !super::tokens::physical(ctx, c, &at, &path, self.wallet).await {
            return None;
        }
        let minimum = if buy {
            units
        } else {
            full_math::mul_div(
                expected,
                U256::from(10_000 - c.slippage),
                U256::from(10_000),
            )?
        };
        let deadline = self.h.timestamp.to::<u64>().checked_add(c.validity)?;
        let order = SwapIntent {
            user: self.wallet,
            recipient: self.wallet,
            tokenIn: input,
            tokenOut: output,
            amountIn: gross,
            minAmountOut: minimum,
            nonce,
            deadline: U256::from(deadline),
            path,
        };
        if !order.matches(self.wallet, &self.markets, self.h.timestamp.to())
            || !canonical(
                ctx,
                &at,
                c.router,
                wire::validatePathCall {
                    path: order.path.clone(),
                },
            )
            .await
            .ok()?
        {
            return None;
        }
        let total = num::to_decimal(if buy { gross } else { expected }, decs.1)?;
        let ticket = Ticket {
            config: c.clone(),
            usd_fee: self.fee,
            order,
            at: quoted_at,
            decimals: decs,
            qty,
            side,
            block: self.h.number.to(),
        };
        if !ticket.valid(Utc::now()) || Config::read(ctx).as_ref() != Some(c) {
            return None;
        }
        Some((ticket, total))
    }
}
