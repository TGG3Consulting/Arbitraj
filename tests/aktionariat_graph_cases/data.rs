use super::*;
pub(super) fn fixture(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}
pub(super) fn number(row: &Value, key: &str) -> U256 {
    row[key].as_str().unwrap().parse().unwrap()
}
pub(super) fn addr(row: &Value, key: &str) -> Address {
    row[key].as_str().unwrap().parse().unwrap()
}
pub(super) fn market(broker: Address, d: &Value, curve: &Value) -> Entry {
    let cash = addr(d, "cash");
    let share = addr(d, "share");
    let mut e = pair(broker, share, cash, 1, 1);
    e.family = families::make("aktionariat-ethereum", "aktionariat").unwrap();
    e.pool.family = e.family.id.into();
    let config: Vec<_> = curve["setting"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .collect();
    let flags = d["settings"].as_u64().unwrap();
    e.pool.state = PoolState::Brokerbot(Box::new(BrokerbotState {
        broker,
        cash,
        share,
        hub: addr(d, "hub"),
        registered: d["registered_pair"].is_array(),
        buying: flags & 1 != 0,
        selling: flags & 2 != 0,
        price: number(curve, "price"),
        base_price: number(curve, "base_price"),
        increment: number(curve, "increment"),
        share_balance: number(d, "shares"),
        cash_balance: number(d, "cash_balance"),
        supply: number(curve, "supply"),
        license_bps: config[8].parse().unwrap(),
        average_first: config[7] == "average",
        drift: curve["drift"].as_bool().unwrap(),
    }));
    e
}
pub(super) fn bridge_pool(reference: Address, coin: Address, counter: Address, r: &Value) -> Entry {
    let mut e = pair(reference, coin, counter, 1, 1);
    e.family = families::make("uniswap_v3", "uniswap_v3").unwrap();
    e.pool.family = e.family.id.into();
    e.pool.kind = e.family.kind;
    assert_eq!(
        (e.pool.token0, e.pool.token1),
        (addr(r, "token0"), addr(r, "token1"))
    );
    let spacing = r["spacing"].as_i64().unwrap() as i32;
    e.pool.state = PoolState::V3 {
        sqrt_price_x96: number(r, "sqrt"),
        tick: r["tick"].as_i64().unwrap() as i32,
        liquidity: number(r, "liquidity").to(),
    };
    e.pool.fee_pips = Some(r["fee"].as_u64().unwrap() as u32);
    e.pool.tick_spacing = Some(spacing);
    let ticks: BTreeMap<_, _> = r["ticks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x[0].as_i64().unwrap() as i32,
                TickData {
                    gross: x[1].as_str().unwrap().parse().unwrap(),
                    net: x[2].as_str().unwrap().parse().unwrap(),
                },
            )
        })
        .collect();
    e.ticks = Some(TickMap {
        spacing,
        ticks: ticks.into(),
        words: (
            r["words"][0].as_i64().unwrap() as i16,
            r["words"][1].as_i64().unwrap() as i16,
        ),
    });
    e
}
