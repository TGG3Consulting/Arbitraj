//! Exact-input кривая DeliHookConstantProduct. Обратный расчёт определяет минимальный
//! вход для той же команды exact-input с заданным порогом выплаты на кошелёк.
use super::{swap_cl::Swapped, v2};
use alloy_primitives::U256;

pub fn quote(reserves: (U256, U256), fee: u32, exact_in: bool, amount: U256) -> Option<Swapped> {
    let (rin, rout) = reserves;
    let den = U256::from(1_000_000);
    if fee >= 1_000_000 || amount.is_zero() || rin.is_zero() || rout.is_zero() {
        return None;
    }
    let factor = U256::from(1_000_000 - fee);
    let input = if exact_in {
        amount
    } else {
        let n = amount.checked_mul(rin)?.checked_mul(den)?;
        let d = rout.checked_sub(amount)?.checked_mul(factor)?;
        if d.is_zero() {
            return None;
        }
        n.div_ceil(d)
    };
    let got = v2::amount_out(input, rin, rout, (factor, den))?;
    if got.is_zero() || (!exact_in && got < amount) {
        return None;
    }
    Some(Swapped {
        amount_in: input,
        amount_out: if exact_in { got } else { amount },
        complete: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use serde_json::Value;

    fn integer(s: &Value) -> U256 {
        s.as_str().unwrap().parse().unwrap()
    }

    #[test]
    fn exact_input_matches_both_wallet_paths_and_the_least_required_input() {
        let src = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/deli_base_wallets.json.gz"
        );
        let data: Value = serde_json::from_reader(GzDecoder::new(src.as_slice())).unwrap();
        let mut accepted = 0;
        for market in data["markets"].as_array().unwrap() {
            let fee = market["key"][2].as_u64().unwrap() as u32;
            for case in market["cases"].as_array().unwrap() {
                for order in ["buy", "sell"].iter().filter_map(|side| case.get(side)) {
                    let res = order["results"].as_array().unwrap();
                    // Перед граничными контролями записаны getReserves хука и quote этой же стороны.
                    let bytes = alloy_primitives::hex::decode(
                        res[res.len() - 8]["returnData"]
                            .as_str()
                            .unwrap()
                            .trim_start_matches("0x"),
                    )
                    .unwrap();
                    let traded = order["calls"].as_array().unwrap();
                    let input_currency = traded[traded.len() - 10]["to"].as_str().unwrap();
                    let even = market["key"][0]
                        .as_str()
                        .unwrap()
                        .eq_ignore_ascii_case(input_currency);
                    let (r0, r1) = (
                        U256::from_be_slice(&bytes[..32]),
                        U256::from_be_slice(&bytes[32..]),
                    );
                    let reserves = if even { (r0, r1) } else { (r1, r0) };
                    let input = integer(&order["input"]);
                    let out = integer(&order["output"]);
                    assert_eq!(quote(reserves, fee, true, input).unwrap().amount_out, out);
                    let least = quote(reserves, fee, false, out).unwrap().amount_in;
                    assert!(least <= input);
                    assert!(
                        quote(reserves, fee, true, least - U256::ONE)
                            .is_none_or(|v| v.amount_out < out)
                    );
                    assert!(
                        matches!((&res[res.len()-6]["status"],&res[res.len()-5]["status"],&res[res.len()-4]["status"]),
                        (a,b,c) if a=="0x0" && b=="0x0" && c=="0x1")
                    );
                    accepted += 1;
                }
            }
        }
        assert_eq!(accepted, 46);
    }

    #[test]
    fn one_atom_exact_division_and_dust_are_distinct() {
        let r = (U256::from(2), U256::from(6));
        assert_eq!(
            quote(r, 0, false, U256::from(2)).unwrap().amount_in,
            U256::ONE
        );
        assert!(quote((U256::from(55), U256::ONE), 6000, true, U256::ONE).is_none());
        assert!(quote(r, 0, false, U256::from(6)).is_none());
    }
}
