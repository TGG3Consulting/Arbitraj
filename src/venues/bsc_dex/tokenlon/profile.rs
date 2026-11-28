//! Адресная пара PMMV5. Количества в профиле всегда относятся к base, в обе стороны.
use super::super::ctx::Ctx;
use crate::settings::scope::Scope;
use alloy_primitives::Address;
use rust_decimal::Decimal;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub(super) struct Market {
    pub base: String,
    pub base_token: Address,
    pub quote: String,
    pub quote_token: Address,
    pub sizes: Vec<Decimal>,
}

fn symbol(text: &str, address: Address) -> bool {
    !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-'))
        && !address.is_zero()
        && ((text == "ETH") == (address == super::wire::WETH))
}

impl Market {
    fn parse(row: &str) -> Option<Self> {
        let mut fields = row.split_whitespace();
        let base = fields.next()?;
        let base_token = fields.next()?.parse().ok()?;
        let quote = fields.next()?;
        let quote_token = fields.next()?.parse().ok()?;
        let mut sizes: Vec<Decimal> = fields.next()?.split('/')
            .map(|s| s.parse::<Decimal>().ok().filter(|v| *v > Decimal::ZERO))
            .collect::<Option<_>>()?;
        if fields.next().is_some() || base == quote || base_token == quote_token
            || !symbol(base, base_token) || !symbol(quote, quote_token) {
            return None;
        }
        sizes.sort();
        sizes.dedup();
        Some(Self { base: base.into(), base_token, quote: quote.into(), quote_token, sizes })
    }
}

pub(super) fn markets(ctx: &Ctx) -> Option<Vec<Market>> {
    if ctx.chain_id()? != 1 {return None;}
    let rows = ctx.settings.get_exact("tokenlon.pairs", &Scope::Venue(ctx.place.id.into()))?;
    let mut seen = HashSet::new();
    rows.as_array()?.iter().map(|row| {
        let market = Market::parse(row.as_str()?)?;
        seen.insert((market.base_token, market.quote_token)).then_some(market)
    }).collect()
}

#[cfg(test)]
#[path="profile_tests.rs"]
mod tests;
