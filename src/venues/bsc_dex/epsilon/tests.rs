//! Повторное чтение исходных calldata и выплат архива без обращения к кошельку.
use super::wire::Plan;
use alloy::rpc::types::Log;
use alloy_primitives::{Address, Bytes, U256, aliases::U24};
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn raw_and_guarded_pay_the_original_owner_on_each_split() {
    let src = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/epsilon_base_wallets.json.gz"
    );
    let data: Value = serde_json::from_reader(GzDecoder::new(src.as_slice())).unwrap();
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let num = |v: &Value| U256::from_str_radix(v.as_str().unwrap(), 10).unwrap();
    let providers = [&data["providers"][0], &data["providers"][1]];
    let tiers = providers.map(|v| v["tier"].as_u64().unwrap());
    let mut counts = [0, 0];
    for row in data["cases"].as_array().unwrap() {
        let protected = match row["mode"].as_str().unwrap() {
            "epsilon" => false,
            "guarded" => true,
            _ => continue,
        };
        let total = num(&row["amount"]);
        let fee = data["feeBps"].as_u64().unwrap();
        let cash = total - total * U256::from(fee) / U256::from(10_000);
        let share = row["firstPercent"].as_u64().unwrap();
        let plan = Plan {
            router: addr(&data["router"]),
            wallet: protected.then(|| addr(&data["wallet"])),
            providers: providers.map(|v| addr(&v["router"])),
            tiers: tiers.map(U24::from),
            pools: providers.map(|v| addr(&v["pools"][v["tier"].to_string()]["pool"])),
            fee_bps: fee.try_into().unwrap(),
            input: addr(&row["input"]),
            output: addr(&row["output"]),
            amount: total,
            minimum: num(&row["received"]),
            first: cash * U256::from(share) / U256::from(100),
            deadline: data["timestamp"].as_u64().unwrap() + 60,
        };
        let (proof, payload) = plan
            .signed(data["chain"].as_u64().unwrap(), addr(&data["owner"]))
            .unwrap();
        let archived: Bytes = row["calldata"].as_str().unwrap().parse().unwrap();
        assert_eq!(payload, archived);
        let mut logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
        assert_eq!(
            proof.amounts(&logs),
            Some((num(&row["spent"]), num(&row["received"])))
        );
        let mark = logs
            .iter()
            .find(|l| l.address() == plan.router && l.topics().len() == 4)
            .unwrap()
            .clone();
        logs.push(mark);
        assert!(proof.amounts(&logs).is_none());
        counts[usize::from(protected)] += 1;
    }
    assert_eq!(counts, [12, 12]);
}
