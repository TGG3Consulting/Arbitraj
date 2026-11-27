//! Match the on-chain controller layout, registry, token orientation and indexed event forms.
use super::*;
use crate::core::types::PoolRef;
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use flate2::read::GzDecoder;

fn data() -> (Value, Value) {
    let primary = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/carbon_registry_ethereum.json"
    ))
    .unwrap();
    let archive =
        include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/carbon_ethereum.json.gz");
    (
        primary,
        serde_json::from_reader(GzDecoder::new(archive.as_slice())).unwrap(),
    )
}
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn number<T: std::str::FromStr>(v: &Value) -> T
where
    T::Err: std::fmt::Debug,
{
    v.as_str().unwrap().parse().unwrap()
}
fn tokens(v: &Value) -> [Address; 2] {
    [addr(&v[0]), addr(&v[1])]
}
fn book(v: &Value) -> BookOrder {
    BookOrder {
        y: number(&v[0]),
        z: number(&v[1]),
        a: number(&v[2]),
        b: number(&v[3]),
    }
}
fn bytes(v: &Value) -> Bytes {
    serde_json::from_value(v.clone()).unwrap()
}
#[test]
fn registry_slots_and_all_strategy_books_match_their_physical_pair() {
    let (primary, archive) = data();
    assert_eq!(primary["selector"], archive["selector"]);
    assert_eq!(primary["state_root"], archive["state_root"]);
    let controller = addr(&archive["roles"][0]);
    let weth: Address = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
        .parse()
        .unwrap();
    let directory = archive["directory"].as_array().unwrap();
    for sample in primary["pairs"].as_array().unwrap() {
        let id = u128::from(sample["id"].as_u64().unwrap());
        let raw_pair = tokens(&directory[id as usize - 1]["tokens"]);
        for (index, slot) in pair_slots(pair_id(id)).iter().enumerate() {
            assert_eq!(
                U256::from_be_bytes(slot.0),
                number::<U256>(&sample["slots"][index])
            );
            assert_eq!(
                raw_pair[index].into_word(),
                number::<B256>(&sample["words"][index])
            );
        }
        for k in ["pair", "reverse"] {
            let result =
                ICarbon::pairCall::abi_decode_returns_validate(&bytes(&sample[k])).unwrap();
            assert_eq!((result.id, result.tokens), (id, raw_pair));
        }
    }
    let positions =
        |id: usize| normalized(tokens(&directory[id - 1]["tokens"]), Some(weth)).unwrap();
    assert_eq!(positions(5).0, positions(602).0);
    assert!(positions(5).1.is_some() && positions(602).1.is_none());
    let mut total = 0;
    let mut reversed = 0;
    for (i, market) in directory.iter().enumerate() {
        let id = i as u128 + 1;
        let physical = tokens(&market["tokens"]);
        for row in market["orders"].as_array().unwrap() {
            let stored = tokens(&row[2]);
            let index = usize::from(stored != physical);
            let row_id = number::<U256>(&row[0]);
            let accepted = orient(
                BookStrategy {
                    id: row_id,
                    owner: addr(&row[1]),
                    tokens: stored,
                    orders: [book(&row[3][0]), book(&row[3][1])],
                },
                id,
                physical,
            )
            .unwrap();
            let expected = book(&row[3][index]);
            assert_eq!(
                (
                    accepted.orders[0].y,
                    accepted.orders[0].z,
                    accepted.orders[0].a,
                    accepted.orders[0].b
                ),
                (expected.y, expected.z, expected.a, expected.b)
            );
            reversed += index;
            total += 1;
        }
    }
    assert_eq!((directory.len(), total), (718, 1596));
    assert!(reversed > 0 && reversed < total);
    for (event, value) in primary["samples"].as_object().unwrap() {
        let log: Log = serde_json::from_value(value.clone()).unwrap();
        let id = if event.starts_with("StrategyCreated(") {
            ICarbon::StrategyCreated::decode_log_data(&log.inner.data)
                .unwrap()
                .id
        } else if event.starts_with("StrategyUpdated(") {
            ICarbon::StrategyUpdated::decode_log_data(&log.inner.data)
                .unwrap()
                .id
        } else {
            continue;
        };
        let id = u128::try_from(id >> 128usize).unwrap();
        assert!((1..=directory.len() as u128).contains(&id));
        assert_eq!(
            super::super::carbon_events::singleton(&log),
            Some(PoolRef::Singleton {
                manager: controller,
                id: pair_id(id)
            })
        );
    }
}
