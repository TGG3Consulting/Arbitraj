//! Файл: tests/multi_common/mod.rs
//! Что делает: общее для тестов многомонетных пулов (решение 295) на ЖИВЫХ записях узла BSC 15.11.2026 — Nerve 3pool
//! (tests/data/nerve_2026-11-15), Ellipsis 3pool (tests/data/ellipsis_2026-11-15): ответ вызова по имени, сборка ответов
//! для разбора загрузки (load_saddle / load_curve: calls → parse) со сверкой запросов байт в байт, decimals монет, пары
//! из coins.json. Тот же разбор, что tests/bsc_dex_stable_list_live.rs (без подмены ответов).
//! Кто вызывает: tests/bsc_dex_multi_pair_golden_stable.rs, tests/bsc_dex_multi_pair*.rs.
//! От чего зависит: serde_json, alloy_primitives, arb::gateway::errors.
#![allow(dead_code)]

use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::gateway::errors::{ErrorCode, GatewayError};

/// Ответы пула по порядку вызовов.
pub type Answers = Vec<Result<Bytes, String>>;

/// Запись из tests/data/<dir>/<file>. Паника — нет файла (тест падает).
pub fn rec(dir: &str, file: &str) -> Value {
    let p = format!("{}/tests/data/{dir}/{file}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON")
}

/// Ответ вызова по имени: Ok(байты) или Err(текст отката). Паника — нет вызова в записи.
pub fn answer(probe: &Value, name: &str) -> Result<Bytes, String> {
    let c = call(probe, name);
    match c["response"]["result"].as_str() {
        Some(h) => Ok(Bytes::from(alloy_primitives::hex::decode(h).unwrap())),
        None => Err(c["response"]["error"]["message"].to_string()),
    }
}

/// Вызов записи по имени. Паника — нет вызова.
fn call<'a>(probe: &'a Value, name: &str) -> &'a Value {
    probe["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("нет вызова {name}"))
}

/// Данные запроса вызова по имени (hex, нижний регистр). Паника — нет вызова.
pub fn request(probe: &Value, name: &str) -> String {
    call(probe, name)["request"]["params"][0]["data"]
        .as_str()
        .unwrap()
        .to_lowercase()
}

/// Слово ответа → U256. Паника — откат.
pub fn word(probe: &Value, name: &str) -> U256 {
    U256::from_be_slice(&answer(probe, name).expect("ответ")[..32])
}

/// Монета записи: тикер → адрес (coins.json).
pub fn coin(coins: &Value, sym: &str) -> Address {
    Address::from_str(coins[sym].as_str().unwrap()).unwrap()
}

/// Ошибка разбора для load_*::parse / build (в боте — ctx.err с площадкой; здесь площадки нет).
pub fn bad(m: String) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, None, m)
}

/// Ответы для load_*::parse: голова — «монета k» / «баланс k» через один до первого отката монеты, хвост — по именам.
/// Запросы головы и хвоста сверяются байт в байт с вызовами load_*::calls. Возвращает: (голова, хвост).
pub fn loader_answers(
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

/// decimals() монет пула по порядку монет (ответ «монета k» — адрес в слове). Принимает: запись, префикс, геттер, N.
pub fn decimals(probe: &Value, pre: &str, getter: &str, n: usize) -> Vec<u8> {
    (0..n)
        .map(|k| {
            let raw = answer(probe, &format!("{pre} {getter}({k})")).unwrap();
            let tok = Address::from_slice(&raw[12..32]).to_checksum(None);
            word(probe, &format!("decimals() {tok}")).to::<u8>()
        })
        .collect()
}

/// Пара пула (token0 < token1) из coins.json — как Found.token0/token1 (discover::sorted).
pub fn pair(dir: &str, a: &str, b: &str) -> (Address, Address) {
    let coins = rec(dir, "coins.json");
    let (x, y) = (coin(&coins, a), coin(&coins, b));
    if x < y { (x, y) } else { (y, x) }
}

/// Три пары 3pool (BUSD, USDC, USDT) в порядке перебора. Ошибок нет.
pub const PAIRS3: [(&str, &str); 3] = [("BUSD", "USDC"), ("BUSD", "USDT"), ("USDC", "USDT")];
