//! Файл: tests/bsc_dex_executor_v3_live.rs
//! Что делает: решение 400 (t185) — приёмка исполнителя v3 по классам шага на записи симуляции живых пулов сети
//! (tests/data/executor_v3_live_2026-11-17, eth_simulateV1 своего узла на H = 134171808, только чтение). Без сети:
//! 1. Сторож: README записи содержит SHA-256 текущего contracts/out/ArbExecutor.bin-runtime; в raw_rpc.json.zst код
//!    исполнителя в stateOverrides КАЖДОЙ симуляции побайтно = текущему bin-runtime и все симуляции — на блоке H.
//! 2. summary.json не на слово: строки = calls.json (case, пул, статус, получено = возврат execute); у (а) / (б) с
//!    got_eq_swap_credit получено = положительная сумма события Swap менеджера этого пула; у (б) квотер «после (а)» —
//!    вызов квотера в ТОЙ ЖЕ симуляции сразу перед execute (б), его ответ = получено (raw_rpc).
//! 3. Правило (live_rule.rs) на записи: приняты ровно Uniswap V4 без крючка и Infinity CL без крючка; счёт пулов по
//!    классам; развёртывание разрешено; шаги непринятых классов (с крючком, Bin, Stable) — отказ admits.
//! 4. Правило строгое на изменённой записи: без SHA в README — ничего не принято; без hooks_reject.json — у CL с
//!    крючком необъяснённые откаты.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::executor::{code, deploy_gate, live_proof, live_rule}, pools::exec_kind,
//! zstd, serde_json.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};
use serde_json::Value;

use arb::venues::bsc_dex::exec::executor::live_rule::{self, Class, Hook};
use arb::venues::bsc_dex::exec::executor::{code, deploy_gate, live_proof};
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;

const DIR: &str = "tests/data/executor_v3_live_2026-11-17";
/// Блок записи H = 134171808 (README записи).
const TAG: &str = "0x7ff4ca0";
/// Адрес X исполнителя в симуляции (README записи: пустой адрес с подменой кода).
const X: &str = "0x00000000000000000000000000000000a4b0e0c3";

/// Текст файла записи. Ошибок нет (паника — файла нет).
fn text(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{DIR}/{name}", env!("CARGO_MANIFEST_DIR"))).expect("запись")
}

/// JSON файла записи. Ошибок нет (паника).
fn json(name: &str) -> Value {
    serde_json::from_str(&text(name)).expect("JSON")
}

/// Сжатая запись raw_rpc.json.zst → JSON. Ошибок нет (паника).
fn raw_rpc() -> Value {
    let p = format!("{}/{DIR}/raw_rpc.json.zst", env!("CARGO_MANIFEST_DIR"));
    let raw = zstd::decode_all(std::fs::File::open(p).expect("запись")).expect("zstd");
    serde_json::from_slice(&raw).expect("JSON")
}

/// Первое 32-байтовое слово hex «0x…» → U256; пусто — None.
fn word(s: &str) -> Option<U256> {
    let h = s.strip_prefix("0x")?;
    let b = alloy_primitives::hex::decode(h.get(..64)?).ok()?;
    Some(U256::from_be_slice(&b))
}

/// Десятичная строка → U256; null — None.
fn dec(v: &Value) -> Option<U256> {
    U256::from_str_radix(v.as_str()?, 10).ok()
}

#[test]
fn record_bound_to_current_code() {
    let cur = code::runtime_sha256().expect("SHA-256 bin-runtime записан");
    assert!(
        text("README.md").contains(cur),
        "README записи — о текущем коде"
    );
    let runtime = std::fs::read_to_string(format!(
        "{}/contracts/out/ArbExecutor.bin-runtime",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let runtime = runtime.trim().trim_start_matches("0x").to_ascii_lowercase();
    let raw = raw_rpc();
    let sims: Vec<&Value> = raw
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["method"] == "eth_simulateV1")
        .collect();
    assert!(!sims.is_empty());
    // Симуляции поиска слотов X не вызывают и кода не подменяют; каждая, что вызывает X, — с текущим bin-runtime.
    let mut with_x = 0;
    for s in &sims {
        assert_eq!(s["params"][1], TAG, "симуляция на блоке H");
        let ov = s["params"][0]["blockStateCalls"][0]["stateOverrides"]
            .as_object()
            .expect("подмены");
        for (a, v) in ov {
            assert!(v.get("state").is_none(), "только stateDiff");
            if let Some(code) = v.get("code").and_then(Value::as_str) {
                assert!(a.eq_ignore_ascii_case(X), "код подменён только у X");
                let code = code.trim_start_matches("0x").to_ascii_lowercase();
                assert_eq!(code, runtime, "в симуляции — текущий bin-runtime");
            }
        }
        let calls = s["params"][0]["blockStateCalls"][0]["calls"]
            .as_array()
            .unwrap();
        if calls
            .iter()
            .any(|c| c["to"].as_str().is_some_and(|t| t.eq_ignore_ascii_case(X)))
        {
            assert!(
                ov.keys().any(|k| k.eq_ignore_ascii_case(X)),
                "вызов X без кода"
            );
            with_x += 1;
        }
    }
    // 33 симуляции с вызовом X — по одной на строку calls.json.
    assert_eq!(with_x, json("calls.json").as_array().unwrap().len());
}

#[test]
fn summary_matches_calls_and_raw_rpc() {
    let summary = json("summary.json");
    let rows: Vec<&Value> = summary
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["case"] != "нет пулов")
        .collect();
    let calls = json("calls.json");
    let calls = calls.as_array().unwrap();
    assert_eq!(rows.len(), calls.len());
    // Пары (ответ квотера, получено execute) — вызов квотера сразу перед execute в той же симуляции после (а).
    let raw = raw_rpc();
    let mut after_a: Vec<(U256, U256)> = Vec::new();
    for s in raw
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["method"] == "eth_simulateV1")
    {
        let req = s["params"][0]["blockStateCalls"][0]["calls"]
            .as_array()
            .unwrap();
        let res = s["result"][0]["calls"].as_array().unwrap();
        let to_x = |i: usize| {
            req[i]["to"]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case(X))
        };
        for i in 1..req.len().saturating_sub(1) {
            let ok = |j: usize| res[j]["status"] == "0x1";
            if to_x(0) && !to_x(i) && to_x(i + 1) && ok(0) && ok(i) && ok(i + 1) {
                let q = word(res[i]["returnData"].as_str().unwrap());
                let g = word(res[i + 1]["returnData"].as_str().unwrap());
                if let (Some(q), Some(g)) = (q, g) {
                    after_a.push((q, g));
                }
            }
        }
    }
    for (r, c) in rows.iter().zip(calls) {
        assert_eq!(r["case"], c["case"]);
        assert_eq!(r["pool_id"], c["pool_id"]);
        assert_eq!(r["status"].as_u64(), c["status"].as_u64());
        if r["status"].as_u64() != Some(1) {
            continue;
        }
        let got = dec(&r["got"]).expect("получено");
        assert_eq!(
            Some(got),
            word(c["returnData"].as_str().unwrap()),
            "получено = возврат execute"
        );
        let single = r["case"] == "a" || r["case"] == "b";
        if single && r["got_eq_swap_credit"] == true {
            let pool = r["pool_id"].as_str().unwrap();
            let manager = r["pool_manager"].as_str().unwrap();
            let credit = c["logs"].as_array().unwrap().iter().any(|l| {
                let at = l["address"]
                    .as_str()
                    .is_some_and(|a| a.eq_ignore_ascii_case(manager));
                let id = l["topics"][1]
                    .as_str()
                    .is_some_and(|t| t.eq_ignore_ascii_case(pool));
                let data = l["data"].as_str().unwrap().trim_start_matches("0x");
                let amt = |k: usize| {
                    data.get(k * 64..k * 64 + 64)
                        .and_then(|w| word(&format!("0x{w}")))
                };
                at && id && [amt(0), amt(1)].contains(&Some(got))
            });
            assert!(credit, "получено = кредит Swap: {pool}");
        }
        if r["case"] == "b"
            && let Some(q) = dec(&r["quoter_out_after_a"])
        {
            assert_eq!(q, got);
            assert!(
                after_a.contains(&(q, got)),
                "квотер после (а) в той же симуляции: {}",
                r["pool_id"]
            );
        }
    }
    // hooks_reject.json — только откаты calls.json с тем же пулом и case.
    for h in json("hooks_reject.json").as_array().unwrap() {
        assert!(
            calls.iter().any(|c| c["pool_id"] == h["pool_id"]
                && c["case"] == h["case"]
                && c["status"] == 0),
            "{h}"
        );
    }
}

/// Класс по виду и крючку (для чтения ожиданий).
fn class(kind: u8, hook: Hook) -> Class {
    Class { kind, hook }
}

#[test]
fn accepted_classes_from_record() {
    let v = live_proof::verdicts().as_ref().expect("запись разобрана");
    let got: BTreeMap<Class, (usize, usize, bool)> = v
        .iter()
        .map(|x| (x.class, (x.pools, x.proven, x.accepted)))
        .collect();
    let want: BTreeMap<Class, (usize, usize, bool)> = [
        (class(19, Hook::Without), (3, 3, true)),
        (class(19, Hook::With), (0, 0, false)),
        (class(20, Hook::Without), (3, 3, true)),
        (class(20, Hook::With), (9, 1, false)),
        (class(20, Hook::Stable), (0, 0, false)),
        (class(21, Hook::Without), (1, 0, false)),
        (class(21, Hook::With), (0, 0, false)),
    ]
    .into_iter()
    .collect();
    assert_eq!(got, want);
    // Bin: откат вне hooks_reject.json — необъяснённое; CL с крючком — класс с крючком не принимается.
    let why = |c: Class| v.iter().find(|x| x.class == c).unwrap().why.clone();
    assert!(why(class(21, Hook::Without)).contains("необъяснённое"));
    assert!(why(class(20, Hook::With)).contains("класс с крючком"));
    assert!(
        !why(class(20, Hook::With)).contains("необъяснённое"),
        "откаты крючков — в hooks_reject"
    );
    // Развёртывание разрешено (принят хотя бы один класс); шаги — по классам.
    assert!(deploy_gate::refusal().is_none());
    let hook = Address::repeat_byte(0x11);
    assert!(live_proof::admits(ExecKind::UniV4, false, Address::ZERO).is_ok());
    assert!(live_proof::admits(ExecKind::InfCl, false, Address::ZERO).is_ok());
    assert!(live_proof::admits(ExecKind::UniV4, false, hook).is_err());
    assert!(live_proof::admits(ExecKind::InfCl, false, hook).is_err());
    assert!(live_proof::admits(ExecKind::InfCl, true, hook).is_err());
    let bin = live_proof::admits(ExecKind::InfBin, false, Address::ZERO).expect_err("Bin");
    assert!(bin.contains("не принят на живых пулах"), "{bin}");
    assert!(
        live_proof::admits(ExecKind::V2, false, Address::ZERO).is_ok(),
        "не шаг менеджера"
    );
    let list = live_proof::listing();
    assert!(
        list.contains("Uniswap V4 (вид 19) без крючка") && list.contains("не приняты"),
        "{list}"
    );
}

#[test]
fn rule_is_strict_on_altered_record() {
    let (summary, reject) = (text("summary.json"), text("hooks_reject.json"));
    let cur = code::runtime_sha256();
    let none = live_rule::judge("README без хеша", &summary, &reject, cur).unwrap();
    assert!(
        none.iter().all(|x| !x.accepted),
        "запись о другом коде — ничего не принято"
    );
    assert!(deploy_gate::refusal_for(&Ok(none)).is_some());
    let bare = live_rule::judge(&text("README.md"), &summary, "[]", cur).unwrap();
    let cl_hook = bare
        .iter()
        .find(|x| x.class == class(20, Hook::With))
        .unwrap();
    assert!(
        cl_hook.why.contains("откат вне hooks_reject.json"),
        "{}",
        cl_hook.why
    );
    assert!(live_rule::judge(&text("README.md"), &summary, &reject, None).is_err());
}
