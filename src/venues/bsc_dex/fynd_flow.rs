use super::raw;
use alloy_primitives::{Address, U256};
use serde_json::Value;
use std::collections::HashMap;

/// Поддерживает поэтапную оплату из накопленного остатка, разделение, слияние и повтор валюты.
/// Доля, возвращённая API, описывает ветвь. Физические суммы атомов уже включают распределение.
pub(super) fn balanced(
    legs: &[Value],
    input: Address,
    output: Address,
    spent: U256,
    gross: U256,
) -> Option<()> {
    let mut coins = HashMap::from([(input, spent)]);
    for leg in legs {
        let from = leg["token_in"].as_str()?.parse::<Address>().ok()?;
        let to = leg["token_out"].as_str()?.parse::<Address>().ok()?;
        let debit = raw(&leg["amount_in"])?;
        let credit = raw(&leg["amount_out"])?;
        if from == to
            || debit.is_zero()
            || credit.is_zero()
            || leg["component_id"].as_str()?.is_empty()
            || leg["protocol"].as_str()?.is_empty()
            || !(0.0..=1.0).contains(
                &leg["split"]
                    .as_f64()
                    .or_else(|| leg["split"].as_str()?.parse::<f64>().ok())?,
            )
        {
            return None;
        }
        raw(&leg["gas_estimate"])?;
        let funds = coins.get_mut(&from)?;
        *funds = funds.checked_sub(debit)?;
        let balance = coins.entry(to).or_default();
        *balance = balance.checked_add(credit)?;
    }
    (coins.remove(&output)? == gross && coins.values().all(|v| v.is_zero())).then_some(())
}

pub(super) struct Net {
    pub router: U256,
    pub client: U256,
    pub minimum: U256,
}
pub(super) fn fees(breakdown: &Value, gross: U256) -> Option<Net> {
    let router = raw(&breakdown["router_fee"])?;
    let client = raw(&breakdown["client_fee"])?;
    let slippage = raw(&breakdown["max_slippage"])?;
    let minimum = gross
        .checked_sub(router)?
        .checked_sub(client)?
        .checked_sub(slippage)?;
    (raw(&breakdown["min_amount_received"])? == minimum).then_some(Net {
        router,
        client,
        minimum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn split_rejoin_conserves_every_atom_and_rejects_foreign_credit() {
        let [a, b, c] = [1, 2, 3].map(Address::repeat_byte);
        let swap = |from: Address, to: Address, deb: u32, cred: u32| {
            json!({"token_in":from,"token_out":to,
            "amount_in":deb.to_string(),"amount_out":cred.to_string(),"gas_estimate":"20","component_id":"pool","protocol":"source","split":0.5})
        };
        let mut legs = vec![swap(a, b, 50, 17), swap(a, c, 50, 20), swap(b, c, 17, 35)];
        legs[2]["split"] = json!("0");
        assert!(balanced(&legs, a, c, U256::from(100), U256::from(55)).is_some());
        legs[2] = swap(b, c, 16, 35);
        assert!(balanced(&legs, a, c, U256::from(100), U256::from(55)).is_none());
        assert!(
            balanced(
                &[swap(a, b, 100, 55)],
                a,
                c,
                U256::from(100),
                U256::from(55)
            )
            .is_none()
        );
        assert!(balanced(&[json!({})], a, c, U256::from(100), U256::from(55)).is_none());
    }
    #[test]
    fn output_fees_are_counted_once_before_slippage() {
        let mut v = json!({"router_fee":"2","client_fee":"1","max_slippage":"3","min_amount_received":"94"});
        assert_eq!(fees(&v, U256::from(100)).unwrap().minimum, U256::from(94));
        v["min_amount_received"] = json!("96");
        assert!(fees(&v, U256::from(100)).is_none());
    }
}
