//! Файл: tests/bsc_dex_stable_list_live.rs
//! Что делает: решение 292 на ЖИВЫХ записях узла BSC (блок 133724547, 15.11.2026; tests/data/nerve_2026-11-15,
//! tests/data/ellipsis_2026-11-15) — без сети и без моков:
//! (кандидаты источника dex.stable_pools — tests/bsc_dex_stable_list_sort.rs, ревью 292)
//! (б) состояние собирает разбор загрузки (load_saddle / load_curve: calls → parse → build — ревью 292, чтобы тест
//!     ловил изменение load_*), своя математика по шаблону из состояния = записанному квотеру пула: Saddle =
//!     calculateSwap, Curve 2020 = get_dy; по 3 суммы на каждую из 6 сторон.
//! Кто вызывает: cargo test --test bsc_dex_stable_list_live.
//! От чего зависит: arb::venues::bsc_dex::pools (load_saddle, load_curve),
//! math::stableswap_v1, arb::gateway::errors.

use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::math::stableswap_v1::{self, Template};
use arb::venues::bsc_dex::pools::{load_curve, load_saddle};

/// Запись из tests/data/<dir>/<file>. Принимает: папку и файл. Возвращает: JSON. Паника — нет файла (тест падает).
fn rec(dir: &str, file: &str) -> Value {
    let p = format!("{}/tests/data/{dir}/{file}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON")
}

/// Ответ вызова по имени: Ok(байты) или Err(текст отката). Паника — нет вызова в записи.
fn answer(probe: &Value, name: &str) -> Result<Bytes, String> {
    let c = probe["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("нет вызова {name}"));
    match c["response"]["result"].as_str() {
        Some(h) => Ok(Bytes::from(alloy_primitives::hex::decode(h).unwrap())),
        None => Err(c["response"]["error"]["message"].to_string()),
    }
}

/// Данные запроса вызова по имени (hex). Паника — нет вызова.
fn request(probe: &Value, name: &str) -> String {
    probe["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["request"]["params"][0]["data"]
        .as_str()
        .unwrap()
        .to_lowercase()
}

/// Слово ответа → U256. Паника — откат.
fn word(probe: &Value, name: &str) -> U256 {
    U256::from_be_slice(&answer(probe, name).expect("ответ")[..32])
}

/// Монеты записи: тикер → адрес.
fn coin(coins: &Value, sym: &str) -> Address {
    Address::from_str(coins[sym].as_str().unwrap()).unwrap()
}

/// Ошибка разбора для load_*::parse / build (в боте — ctx.err с площадкой; здесь площадки нет).
fn bad(m: String) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, None, m)
}

/// Ответы пула по порядку вызовов.
type Answers = Vec<Result<Bytes, String>>;

/// Ответы для load_*::parse: голова — «монета k» / «баланс k» через один до первого отката монеты (дальше разбор не
/// смотрит), хвост — по именам. Запросы головы и хвоста сверяются байт в байт с вызовами load_*::calls.
fn loader_answers(
    probe: &Value,
    calls: &[(Address, Bytes)],
    tail: usize,
    names: [&str; 2],
    tail_names: &[&str],
) -> (Answers, Answers) {
    let hex = |b: &Bytes| format!("0x{}", alloy_primitives::hex::encode(b));
    let mut head = Vec::new();
    for k in 0.. {
        let n = format!("{}({k})", names[0]);
        assert_eq!(hex(&calls[2 * k].1), request(probe, &n));
        let a = answer(probe, &n);
        let stop = a.is_err();
        head.push(a);
        if stop {
            break;
        }
        let b = format!("{}({k})", names[1]);
        assert_eq!(hex(&calls[2 * k + 1].1), request(probe, &b));
        head.push(answer(probe, &b));
    }
    let tail_ans = tail_names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            assert_eq!(hex(&calls[tail + i].1), request(probe, n));
            answer(probe, n)
        })
        .collect();
    (head, tail_ans)
}

/// decimals() монет пула по порядку монет состояния (ответ «монета k» — адрес в слове).
fn decimals(probe: &Value, pre: &str, getter: &str, n: usize) -> Vec<u8> {
    (0..n)
        .map(|k| {
            let raw = answer(probe, &format!("{pre} {getter}({k})")).unwrap();
            let tok = Address::from_slice(&raw[12..32]).to_checksum(None);
            word(probe, &format!("decimals() {tok}")).to::<u8>()
        })
        .collect()
}

/// (б) состояние, собранное разбором загрузки, × своя математика по шаблону из состояния = квотер пула.
/// Принимает: запись, состояние, префикс и имя квотера. Возвращает: число совпавших котировок.
fn math_case(probe: &Value, st: &PoolState, pre: &str, q: &str) -> usize {
    let PoolState::StableSwap(d) = st else {
        panic!("не StableSwap")
    };
    let t = Template {
        amp_precision: d.amp_precision,
        max_rounds: d.max_rounds,
        strict: d.strict,
        pair_d: d.pair_d,
    };
    assert!(!d.halted);
    let mut n = 0;
    for c in probe["calls"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let Some(args) = name.strip_prefix(&format!("{pre} {q}(")) else {
            continue;
        };
        let v: Vec<&str> = args.trim_end_matches(')').split(',').collect();
        let (i, j): (usize, usize) = (v[0].parse().unwrap(), v[1].parse().unwrap());
        let dx: U256 = v[2].parse().unwrap();
        let got = stableswap_v1::exchange_out_t(i, j, dx, &d.balances, &d.rates, d.amp, d.fee, &t)
            .unwrap();
        assert_eq!(got, word(probe, name), "{name}");
        n += 1;
    }
    n
}

/// Пара пула (token0 < token1) из coins.json — как Found.token0/token1 (discover::sorted).
fn pair(dir: &str, a: &str, b: &str) -> (Address, Address) {
    let coins = rec(dir, "coins.json");
    let (x, y) = (coin(&coins, a), coin(&coins, b));
    if x < y { (x, y) } else { (y, x) }
}

#[test]
fn nerve_math_matches_live_calculate_swap() {
    let dir = "nerve_2026-11-15";
    let probe = rec(dir, "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let (calls, tail) = load_saddle::calls(pool);
    let names = ["nerve getToken", "nerve getTokenBalance"];
    let tails = [
        "nerve getAPrecise()",
        "nerve swapStorage()",
        "nerve paused()",
    ];
    let (head, t) = loader_answers(&probe, &calls, tail, names, &tails);
    let p = load_saddle::parse(&head, &t, pair(dir, "BUSD", "USDT"), &bad).unwrap();
    let dec = decimals(&probe, "nerve", "getToken", p.coins.len());
    let st = load_saddle::build(p, &dec, &bad).unwrap();
    assert_eq!(math_case(&probe, &st, "nerve", "calculateSwap"), 18);
}

#[test]
fn ellipsis_math_matches_live_get_dy() {
    let dir = "ellipsis_2026-11-15";
    let probe = rec(dir, "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let (calls, tail) = load_curve::calls(pool);
    let names = ["ellipsis coins", "ellipsis balances"];
    let tails = ["ellipsis A()", "ellipsis fee()"];
    let (head, t) = loader_answers(&probe, &calls, tail, names, &tails);
    let (coins, balances, amp, fee, i0, i1) =
        load_curve::parse(&head, &t, pair(dir, "USDC", "USDT"), &bad).unwrap();
    let dec = decimals(&probe, "ellipsis", "coins", coins.len());
    let st = load_curve::build(balances, amp, fee, (i0, i1), &dec, &bad).unwrap();
    assert_eq!(math_case(&probe, &st, "ellipsis", "get_dy"), 18);
}
