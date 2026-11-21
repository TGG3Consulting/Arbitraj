//! Файл: tests/bsc_dex_stable_list_sort.rs
//! Что делает: ревью 292 — разбор ответов пачки источника dex.stable_pools (pools/stable_list.rs sort) на записанных
//! ответах узла BSC (tests/data/stable_list_2026-11-15): все вызовы token_calls двух пулов (Nerve 3pool getToken(0..31),
//! Ellipsis 3pool coins(0..7); запросы = token_calls байт в байт) — кандидаты у каждого пула свои (BUSD/USDT, USDC/USDT;
//! BTCB — монета не из пула — не даёт пар); сбой узла на части пачки у первого пула (настоящий
//! отказ узла — node_error.json; try_chunks ставит Err на все вызовы части, node/call.rs) не сдвигает ответы второго:
//! первый — без кандидатов и в no_answer, второй — свои кандидаты со своим адресом.
//! Кто вызывает: cargo test --test bsc_dex_stable_list_sort.
//! От чего зависит: arb::venues::bsc_dex::pools (stable_list, families), node::call (Piece).

use std::collections::HashMap;
use std::str::FromStr;

use alloy_primitives::{Address, Bytes};
use serde_json::Value;

use arb::venues::bsc_dex::node::call::Piece;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::stable_list::{self, Extra};

const DIR: &str = "stable_list_2026-11-15";

/// Запись из tests/data/<DIR>/<file>. Паника — нет файла.
fn rec(file: &str) -> Value {
    let p = format!("{}/tests/data/{DIR}/{file}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON")
}

/// Монета из tests/data/nerve_2026-11-15/coins.json.
fn coin(sym: &str) -> Address {
    let p = format!(
        "{}/tests/data/nerve_2026-11-15/coins.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    Address::from_str(v[sym].as_str().unwrap()).unwrap()
}

/// Пулы и ответы по порядку token_calls: Ok(Ok) — результат, Ok(Err) — откат (текст узла). Запросы записи =
/// вызовам token_calls байт в байт.
fn recorded() -> (Vec<(&'static families::Family, Address)>, Vec<Piece>) {
    let r = rec("token_calls.json");
    let calls = r["calls"].as_array().unwrap();
    let nerve = families::make("nerve", "saddle_stableswap").unwrap();
    let ell = families::make("ellipsis", "curve_stableswap").unwrap();
    let addr = |i: usize| {
        Address::from_str(calls[i]["request"]["params"][0]["to"].as_str().unwrap()).unwrap()
    };
    let pools = vec![(nerve, addr(0)), (ell, addr(32))];
    let want: Vec<(Address, Bytes)> = pools
        .iter()
        .flat_map(|(f, p)| stable_list::token_calls(f.proto, *p))
        .collect();
    assert_eq!(want.len(), calls.len());
    let mut pieces = Vec::new();
    for (c, (to, data)) in calls.iter().zip(&want) {
        assert_eq!(
            *to,
            Address::from_str(c["request"]["params"][0]["to"].as_str().unwrap()).unwrap()
        );
        assert_eq!(
            format!("0x{}", alloy_primitives::hex::encode(data)),
            c["request"]["params"][0]["data"]
                .as_str()
                .unwrap()
                .to_lowercase()
        );
        pieces.push(Ok(match c["response"]["result"].as_str() {
            Some(h) => Ok(Bytes::from(alloy_primitives::hex::decode(h).unwrap())),
            None => Err(c["response"]["error"]["message"]
                .as_str()
                .unwrap()
                .to_string()),
        }));
    }
    (pools, pieces)
}

/// Подписка: BUSD, USDC (монеты), встречный USDT.
fn subs() -> (HashMap<Address, String>, Vec<Address>) {
    let m = [("BUSD", coin("BUSD")), ("USDC", coin("USDC"))]
        .into_iter()
        .map(|(s, a)| (a, s.to_string()))
        .collect();
    (m, vec![coin("USDT")])
}

/// Пары (монета, встречный) кандидатов пула.
fn of(x: &Extra, pool: Address) -> Vec<(Address, Address)> {
    let key = format!("{pool:#x}");
    let mut v: Vec<_> = x
        .found
        .values()
        .flatten()
        .filter(|p| p.pool == key)
        .map(|p| (p.base, p.quote))
        .collect();
    v.sort();
    v
}

#[test]
fn all_answers_give_each_pool_its_candidates() {
    let (pools, pieces) = recorded();
    let (coins, quotes) = subs();
    let mut coins = coins;
    // BTCB — настоящая монета (документация Nerve), которой нет в пулах: кандидатов с ней нет.
    coins.insert(coin("BTCB"), "BTCB".to_string());
    let x = stable_list::sort(&pools, pieces, &coins, &quotes);
    let mut want = vec![(coin("BUSD"), coin("USDT")), (coin("USDC"), coin("USDT"))];
    want.sort();
    assert_eq!(of(&x, pools[0].1), want);
    assert_eq!(of(&x, pools[1].1), want);
    assert_eq!(x.no_answer, 0);
    // Монета не из пула не даёт пары и как встречный токен; пара строится в любую сторону.
    let toks = [coin("BUSD"), coin("USDT"), coin("USDC")];
    assert!(stable_list::pairs(&toks, &[coin("BUSD")], &[coin("BTCB")]).is_empty());
    let pair = stable_list::pairs(&toks, &[coin("USDT")], &[coin("BUSD")]);
    assert_eq!(pair, vec![(coin("USDT"), coin("BUSD"))]);
}

#[test]
fn node_failure_in_first_pool_does_not_shift_second() {
    let (pools, mut pieces) = recorded();
    let node = rec("node_error.json");
    let why = node["response"]["error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    // Части пачки по 4 вызова: части 2..8 первого пула (вызовы 4..31) — отказ узла, у второго пула ответы целы.
    for p in pieces.iter_mut().take(32).skip(4) {
        *p = Err(why.clone());
    }
    let (coins, quotes) = subs();
    let x = stable_list::sort(&pools, pieces, &coins, &quotes);
    assert!(
        of(&x, pools[0].1).is_empty(),
        "у пула со сбоем узла кандидатов нет"
    );
    assert_eq!(x.no_answer, 32);
    let mut want = vec![(coin("BUSD"), coin("USDT")), (coin("USDC"), coin("USDT"))];
    want.sort();
    assert_eq!(of(&x, pools[1].1), want, "второй пул — свои ответы");
}
