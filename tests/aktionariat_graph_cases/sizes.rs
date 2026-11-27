//! Подбор размера на реальных ценах CHF, включая частичный допуск после измерения одной акции.
use super::*;
use arb::core::types::{Side, TokenTaxes};
use arb::venues::bsc_dex::{
    check::taxes_util,
    num,
    quote::{graph_split, route_best, route_calc, select::Usable, side_fit},
};
use rust_decimal::Decimal;

pub(super) fn verify(
    p: &Parts,
    markets: &[Entry],
    row: &Value,
    usd: Address,
    wrapped: Address,
    chf: Address,
    stock: Address,
) {
    let ctx = &p.ctx;
    let qty = Decimal::from(row["units"].as_u64().unwrap());
    let budget = num::to_decimal(number(row, "budget"), 6).unwrap();
    let owner_budget = ctx.dec("check.probe_usdt").unwrap();
    assert_eq!(side_fit::min_units(ctx).unwrap(), U256::from(910));
    let (accepted, _, _) = taxes_util::probe_pools(ctx, &p.registry, &p.tokens, &stock, 6).unwrap();
    if qty == Decimal::ONE {
        assert_eq!(
            accepted.iter().any(|(e, a)| e.coin == stock && a.is_some()),
            budget <= owner_budget,
            "{stock}: budget {budget}"
        );
    }
    let zero = Decimal::ZERO;
    let taxes = TokenTaxes {
        buy_pct: zero,
        sell_pct: zero,
        transfer_pct: zero,
    };
    let edges = [
        (usd, wrapped, 6, 18),
        (wrapped, chf, 18, 18),
        (chf, stock, 18, 0),
    ];
    let mut legs: Vec<_> = markets
        .iter()
        .zip(edges)
        .map(
            |(entry, (token_in, token_out, dec_in, dec_out))| route_calc::Leg {
                entry: entry.clone(),
                token_in,
                token_out,
                dec_in,
                dec_out,
                usable: Usable {
                    one_share_only: false,
                    gas: 0,
                    tax_pct: zero,
                    hook_pct: zero,
                    hook: Default::default(),
                },
            },
        )
        .collect();
    for limited in [false, true] {
        legs[2].usable.one_share_only = limited;
        if limited && qty != Decimal::ONE {
            assert!(route_calc::eval(ctx, &legs, Side::Buy, qty, Decimal::ONE).is_err());
            continue;
        }
        let candidates = vec![graph_split::Cand::new(legs.clone(), zero)];
        let mut best = route_best::best(ctx, candidates, Side::Buy, qty, &taxes, 4).unwrap();
        let paths = graph_split::split(ctx, &mut best, Side::Buy, qty, &taxes, false).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!((paths[0].coins, paths[0].usdt), (qty, budget));
    }
}
