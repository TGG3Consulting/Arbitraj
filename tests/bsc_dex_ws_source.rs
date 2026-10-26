//! Файл: tests/bsc_dex_ws_source.rs
//! Что делает: источник событий пулов «подписка logs по WebSocket» (решение 79) на ЖИВЫХ записях публичного узла
//! wss://bsc-rpc.publicnode.com (tests/data/ws_logs_2026-10-26, записал техлид рекордером record_ws_logs.js):
//! - кадр eth_subscription — тот же формат, что лог квитанции (address, topics, blockNumber, logIndex, removed), и
//!   фильтр, который строит адаптер для V2/V3 (node/subs.rs by_address), доставляет настоящий записанный кадр
//!   (node/log_filter.rs — тот же разбор фильтра, что у ленты квитанций);
//! - CL/V4: фильтр by_pool_id кладёт наши PoolId в topics[1] у адреса менеджера — лог менеджера с нашим PoolId
//!   проходит, с чужим — нет (отсев чужих пулов у узла, вариант (а) решения 79);
//! - пределы из записи (материал дизайна): один фильтр принял до 5000 адресов, topics[1] — до 5000 PoolId на обоих
//!   менеджерах, 10 соединений на IP держатся — значит дробить ~2850 пулов не нужно (одна by_address на вид +
//!   по одной by_pool_id на менеджера), подписок единицы.
//!
//! Селекторы событий берём из пути квитанций (официальные ABI, pools/events*.rs) — эта запись подтверждает доставку и
//! формат, не полный набор топиков. removed:true в записях не встретился (реоргов за окно не было) — известный пробел
//! покрытия, как у depth; ловится на длинной записи.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::node::{subs, log_filter}, serde_json.

use alloy_primitives::{Address, B256};
use arb::venues::bsc_dex::node::log_filter::Filter;
use arb::venues::bsc_dex::node::subs::{by_address, by_pool_id};
use serde_json::{Value, json};

const MAIN: &str = "tests/data/ws_logs_2026-10-26/responses.json";
const MGR: &str = "tests/data/ws_logs_2026-10-26/responses_mgr.json";

fn load(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Минимальный eth-лог из записанного кадра (log_filter читает только address и topics).
fn log_of(frame: &Value) -> Value {
    json!({
        "address": frame["address"],
        "topics": frame["topics"],
        "blockNumber": frame["blockNumber"],
        "logIndex": frame["logIndex"],
        "transactionHash": frame["txHash"],
        "removed": frame["removed"],
    })
}

#[test]
fn our_address_filter_delivers_recorded_frames() {
    let rec = load(MAIN);
    let frames = rec["frames"].as_array().unwrap();
    assert!(!frames.is_empty(), "в записи есть кадры");
    for f in frames {
        let addr: Address = f["address"].as_str().unwrap().parse().unwrap();
        let t0: B256 = f["topic0"].as_str().unwrap().parse().unwrap();
        // Фильтр адаптера для этого пула (by_address) — тот же, что пошлёт ws-подписка; он доставляет настоящий кадр.
        let filter = Filter::new(&by_address(&[addr], &[t0]));
        assert!(
            filter.matches(&log_of(f)),
            "кадр {} не прошёл свой фильтр",
            f["txHash"]
        );
        // Чужой адрес в фильтре — кадр не наш, не доставляется.
        let other: Address = "0x000000000000000000000000000000000000dead"
            .parse()
            .unwrap();
        assert!(!Filter::new(&by_address(&[other], &[t0])).matches(&log_of(f)));
    }
}

#[test]
fn manager_pool_id_filter_selects_only_our_pools() {
    // Форма фильтра CL/V4: {address: менеджер, topics: [[topic0], [наши PoolId]]}.
    let mgr: Address = "0x28e2ea090877bf75740558f6bfb36a5ffee9e9df"
        .parse()
        .unwrap();
    let t0: B256 = "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f"
        .parse()
        .unwrap(); // Uniswap V4 Swap (встречен в файрхозе записи)
    let ours: B256 = "0xbe42d277f1865d14f5787bff0930099d9f4d3748eedaa2e983aa0bae7c36d670"
        .parse()
        .unwrap();
    let alien: B256 = "0x1111111111111111111111111111111111111111111111111111111111111111"
        .parse()
        .unwrap();
    let f = by_pool_id(mgr, &[ours], &[t0]);
    assert_eq!(f["address"], json!(mgr));
    assert_eq!(f["topics"][0], json!([t0]));
    assert_eq!(f["topics"][1], json!([ours]));
    let filter = Filter::new(&f);
    let log = |id: B256| json!({ "address": mgr, "topics": [t0, id] });
    assert!(
        filter.matches(&log(ours)),
        "лог менеджера с нашим PoolId проходит"
    );
    assert!(!filter.matches(&log(alien)), "чужой PoolId не проходит");
}

#[test]
fn recording_confirms_no_sharding_needed() {
    // Адреса в одном фильтре — все размеры приняты (main-запись).
    for l in load(MAIN)["limits"].as_array().unwrap() {
        if l["type"] == "addresses_per_filter" {
            assert_eq!(l["ok"], json!(true), "фильтр отверг {} адресов", l["size"]);
        }
    }
    // PoolId в topics[1] — все размеры приняты на обоих менеджерах; соединений держится ≥ 3 (нам нужно ~3 подписки).
    let mgr = load(MGR);
    let mut poolid_sizes = 0;
    for l in mgr["limits"].as_array().unwrap() {
        match l["type"].as_str() {
            Some("poolids_in_topic1") => {
                assert_eq!(
                    l["ok"],
                    json!(true),
                    "topics[1] отверг {} PoolId ({})",
                    l["size"],
                    l["kind"]
                );
                poolid_sizes += 1;
            }
            Some("connections") => {
                assert!(
                    l["alive_after_hold"].as_u64().unwrap() >= 3,
                    "на IP держится < 3 соединений"
                );
            }
            _ => {}
        }
    }
    assert!(
        poolid_sizes >= 2,
        "в записи есть пробы topics[1] обоих менеджеров"
    );
}
