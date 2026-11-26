use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;

fn num(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn mint_and_burn_match_both_archived_wallets_and_invert_to_adjacent_unit() {
    let base = &include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/mint_club_base_wallets.json.gz"
    )[..];
    let eth = &include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/mint_club_ethereum_wallets.json.gz"
    )[..];
    let mut cases = 0;
    for archive in [base, eth] {
        let data: Value = serde_json::from_reader(GzDecoder::new(archive)).unwrap();
        for m in data["markets"].as_array().unwrap() {
            let usd = m["reserve"].as_str().unwrap().parse().unwrap();
            let asset = m["token"].as_str().unwrap().parse().unwrap();
            let s = MintClubState {
                bond: data["bond"].as_str().unwrap().parse().unwrap(),
                token: asset,
                reserve: usd,
                supply: num(&m["supply"]),
                steps: m["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| {
                        (
                            row[0].as_str().unwrap().parse().unwrap(),
                            row[1].as_str().unwrap().parse().unwrap(),
                        )
                    })
                    .collect(),
                mint_bps: m["mint_bps"].as_u64().unwrap() as u16,
                burn_bps: m["burn_bps"].as_u64().unwrap() as u16,
                bonded: num(&m["bonded"]),
                available: num(&m["bonded"]),
            };
            for row in m["trials"].as_array().unwrap() {
                let size = num(&row["size"]);
                let cost = num(&row["mint_cost"]);
                let returned = num(&row["redeem"]);
                let mint = fill(&s, usd, false, size).unwrap();
                assert_eq!(mint.swapped.amount_in, cost);
                let by_budget = fill(&s, usd, true, cost).unwrap().swapped;
                assert!(by_budget.amount_out >= size && by_budget.amount_in <= cost);
                assert!(
                    fill(&s, usd, false, by_budget.amount_out + U256::ONE)
                        .is_none_or(|f| f.swapped.amount_in > cost)
                );
                let mut post = s.clone();
                post.supply += size;
                post.bonded += mint.gross;
                post.available += cost;
                let burn = fill(&post, asset, true, size).unwrap();
                assert_eq!(burn.swapped.amount_out, returned);
                let min_units = fill(&post, asset, false, returned)
                    .unwrap()
                    .swapped
                    .amount_in;
                assert!(min_units <= size);
                assert!(
                    fill(&post, asset, true, min_units - U256::ONE)
                        .is_none_or(|f| f.swapped.amount_out < returned)
                );
                let (_, next) = after(&post, asset, size).unwrap();
                assert_eq!(next.supply, s.supply);
                assert_eq!(next.bonded, post.bonded - burn.gross);
                assert_eq!(next.available, post.available - returned);
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 17);
}

#[test]
fn exact_inputs_match_exhaustive_small_curve_with_free_and_royalty_steps() {
    let unit = u128::from(UNIT);
    let token = Address::repeat_byte(1);
    let reserve = Address::repeat_byte(2);
    let mut s = MintClubState {
        bond: Address::repeat_byte(3),
        token,
        reserve,
        supply: U256::ZERO,
        steps: vec![(3, 0), (8, unit / 3), (32, 7 * unit / 2)],
        mint_bps: 1750,
        burn_bps: 2200,
        bonded: U256::from(100),
        available: U256::from(100),
    };
    for supply in 0..=32u64 {
        s.supply = U256::from(supply);
        for raw in 1..=65u64 {
            let amount = U256::from(raw);
            let max = (1..=32 - supply)
                .filter(|n| {
                    fill(&s, reserve, false, U256::from(*n))
                        .is_some_and(|f| f.swapped.amount_in <= amount)
                })
                .max();
            assert_eq!(
                fill(&s, reserve, true, amount).map(|f| f.swapped.amount_out),
                max.map(U256::from)
            );
            let min = (1..=supply).find(|n| {
                fill(&s, token, true, U256::from(*n))
                    .is_some_and(|f| f.swapped.amount_out >= amount)
            });
            assert_eq!(
                fill(&s, token, false, amount).map(|f| f.swapped.amount_in),
                min.map(U256::from)
            );
        }
    }
    s.bonded = U256::ONE;
    assert!(fill(&s, token, true, U256::from(2)).is_none());
    s.bonded = U256::from(100);
    s.available = U256::ONE;
    assert!(fill(&s, token, true, U256::from(2)).is_none());
}
