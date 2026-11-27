//! Официальная глубина Base. Каждый уровень задаёт отдельное количество базового токена.
//! https://docs.kipseli.capital/openapi.yaml
use crate::venues::bsc_dex::{
    math::{
        full_math::{mul_div, mul_div_up},
        ten,
    },
    num,
};
use alloy_primitives::{Address, U256, U512};
use serde_json::Value;

fn rate(raw: &str, base: u8, cash: u8) -> Option<(U256, U256)> {
    let digits = raw
        .split_once('.')
        .map_or(0, |(_, f)| f.trim_end_matches('0').len());
    let scale = u8::try_from(digits).ok()?;
    let value = num::exact_amount(None, Some(raw), scale)?;
    let shift = i32::from(cash) - i32::from(base) - i32::from(scale);
    let factor = ten::checked(shift.unsigned_abs())?;
    if shift >= 0 {
        Some((value.checked_mul(factor)?, U256::ONE))
    } else {
        Some((value, factor))
    }
}

pub(super) fn amounts(
    book: &Value,
    input: Address,
    output: Address,
    dec_in: u8,
    dec_out: u8,
    requested: U256,
) -> Option<(U256, U256)> {
    let pair = |row: &Value| -> Option<(Address, Address)> {
        Some((
            row["baseToken"].as_str()?.parse().ok()?,
            row["quoteToken"].as_str()?.parse().ok()?,
        ))
    };
    let mut rows = book["pairs"]
        .as_array()?
        .iter()
        .filter(|p| pair(p).is_some_and(|t| t == (input, output) || t == (output, input)));
    let row = rows.next()?;
    if rows.next().is_some() {
        return None;
    }
    let sell = pair(row)?.0 == input;
    let (base, cash, side) = if sell {
        (dec_in, dec_out, "bids")
    } else {
        (dec_out, dec_in, "asks")
    };
    let mut remaining = requested;
    let mut received = U256::ZERO;
    let mut previous = None;
    for level in row[side].as_array()? {
        let (n, d) = rate(level["price"].as_str()?, base, cash)?;
        if let Some((old_n, old_d)) = previous {
            let current = U512::from(n) * U512::from(old_d);
            let old = U512::from(old_n) * U512::from(d);
            if (sell && current > old) || (!sell && current < old) {
                return None;
            }
        }
        previous = Some((n, d));
        let available = num::exact_amount(None, level["amount"].as_str(), base)?;
        let (paid, out) = if sell {
            let take = remaining.min(available);
            (take, mul_div(take, n, d)?)
        } else {
            let take = mul_div(remaining, d, n)?.min(available);
            (mul_div_up(take, n, d)?, take)
        };
        if out.is_zero() {
            break;
        }
        remaining = remaining.checked_sub(paid)?;
        received = received.checked_add(out)?;
        if remaining.is_zero() {
            break;
        }
    }
    (!received.is_zero()).then_some((requested - remaining, received))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn marginal_partial_levels_and_ask_rounding() {
        let base = Address::with_last_byte(1);
        let cash = Address::with_last_byte(2);
        let book = json!({"pairs":[{"baseToken":base,"quoteToken":cash,
        "bids":[{"price":"3","amount":"2"},{"price":"2","amount":"1"}],
        "asks":[{"price":"3.5","amount":"2"},{"price":"4","amount":"1"}]}]});
        let raw = U256::from;
        assert_eq!(
            amounts(&book, base, cash, 0, 0, raw(4u64)),
            Some((raw(3), raw(8)))
        );
        assert_eq!(
            amounts(&book, cash, base, 0, 0, raw(8)),
            Some((raw(7), raw(2)))
        );
    }
}
