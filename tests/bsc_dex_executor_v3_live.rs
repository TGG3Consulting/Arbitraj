//! Файл: tests/bsc_dex_executor_v3_live.rs
//! Что делает: решение 400 (t185) — приёмка исполнителя v3 по классам шага на записи симуляции живых пулов сети
//! (tests/data/executor_v3_live_2026-11-17, eth_simulateV1 своего узла на H = 134171808, только чтение). Без сети:
//! 1. Сторож: README записи содержит SHA-256 текущего contracts/out/ArbExecutor.bin-runtime; в raw_rpc.json.zst код
//!    исполнителя в stateOverrides КАЖДОЙ симуляции побайтно = текущему bin-runtime и все симуляции — на блоке H.
//! 2. summary.json не на слово: строки = calls.json (case, пул, статус, получено = возврат execute); у (а) / (б) с
//!    got_eq_swap_credit получено = положительная сумма события Swap менеджера этого пула; у (б) квотер «после (а)» —
//!    вызов квотера в ТОЙ ЖЕ симуляции сразу перед execute (б), его ответ = получено (raw_rpc).
//! 3. Правило (live_rule.rs) на записи: приняты ровно Uniswap V4 без крючка и Infinity CL без крючка; счёт пулов по
//!    классам; развёртывание кода v3 в сети BSC разрешено; шаги непринятых классов (с крючком, Bin, Stable) — отказ
//!    admits_for. Решение 461: запись — пары (v3, chainId 56); v3 в Arbitrum — записи нет, «не проверен». Запись
//!    текущей сборки v4 — tests/bsc_dex_executor_v4_live.rs.
//! 4. Правило строгое на изменённой записи: без SHA в README — ничего не принято; без hooks_reject.json — у CL с
//!    крючком необъяснённые откаты.
//! 5. Ревью t185: остаток на исполнителе = 0 и квотер (а) на H — по raw_rpc (не по сводке); строгость на изменённой
//!    копии (класс Uniswap V4 без крючка перестаёт приниматься): откат пула без крючка, «объяснённый» в hooks_reject;
//!    x_left пустой / без ключа токена; получено = квотер = 0; SHA не в строке «sha256 симулированного bin-runtime:».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::executor::{code, deploy_gate, live_proof, live_rule}, pools::exec_kind,
//! zstd, serde_json.

use std::collections::BTreeMap;

use alloy_primitives::{Address, U256};
use serde_json::Value;

use arb::venues::bsc_dex::exec::executor::live_rule::{self, Class, Hook};
use arb::venues::bsc_dex::exec::executor::code::{self, Version};
use arb::venues::bsc_dex::exec::executor::{deploy_gate, live_proof};
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;

const DIR: &str = "tests/data/executor_v3_live_2026-11-17";
/// Блок записи H = 134171808 (README записи).
const TAG: &str = "0x7ff4ca0";
/// Адрес X исполнителя в симуляции (README записи: пустой адрес с подменой кода).
const X: &str = "0x00000000000000000000000000000000a4b0e0c3";
/// Сеть записи — свой узел BSC (README записи: «WBNB (dex.native_wrapped, bsc_dex)»; решение 461 — пара (v3, 56)).
const BSC: Option<u64> = Some(56);
/// Сеть Arbitrum One — записи v3 в ней нет (решение 461).
const ARB: Option<u64> = Some(42161);

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
    let cur = code::runtime_sha256_of(Version::V3).expect("SHA-256 bin-runtime записан");
    assert!(
        text("README.md").contains(cur),
        "README записи — о текущем коде"
    );
    let runtime = std::fs::read_to_string(format!(
        "{}/contracts/out/v3/ArbExecutor.bin-runtime",
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

/// Решение 461: admits_for записи пары (v3, BSC). Ошибок нет.
fn v3(k: ExecKind, stable: bool, h: Address) -> Result<(), String> {
    live_proof::admits_for(Version::V3, BSC, k, stable, h)
}

#[test]
fn accepted_classes_from_record() {
    let v = live_proof::verdicts_of(Version::V3, BSC)
        .as_ref()
        .expect("запись разобрана");
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
        // Решение 441 (ревью, п. 2): шага BALANCER_V3 в записи нет — не принят.
        (class(23, Hook::Without), (0, 0, false)),
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
    // Для кода v3 в сети BSC запись разрешает развёртывание (принят хотя бы один класс); шаги — по классам. Решение
    // 461: запись — пары (v3, chainId 56); у v3 в другой сети (Arbitrum) записи нет — все классы «не проверен».
    let listed = live_proof::verdicts_of(Version::V3, BSC);
    assert!(deploy_gate::refusal_for(Version::V3, BSC, listed).is_none());
    let hook = Address::repeat_byte(0x11);
    assert!(v3(ExecKind::UniV4, false, Address::ZERO).is_ok());
    assert!(v3(ExecKind::InfCl, false, Address::ZERO).is_ok());
    assert!(v3(ExecKind::UniV4, false, hook).is_err());
    assert!(v3(ExecKind::InfCl, false, hook).is_err());
    assert!(v3(ExecKind::InfCl, true, hook).is_err());
    assert!(v3(ExecKind::BalancerV3, false, Address::ZERO).is_err());
    let bin = v3(ExecKind::InfBin, false, Address::ZERO).expect_err("Bin");
    assert!(bin.contains("не принят на живых пулах"), "{bin}");
    assert!(
        v3(ExecKind::V2, false, Address::ZERO).is_ok(),
        "не шаг менеджера"
    );
    let list = live_proof::listing_for(Version::V3, BSC);
    assert!(
        list.contains("Uniswap V4 (вид 19) без крючка") && list.contains("не приняты"),
        "{list}"
    );
    assert!(list.contains(DIR), "{list}");
    let other = live_proof::admits_for(Version::V3, ARB, ExecKind::UniV4, false, Address::ZERO);
    let other = other.expect_err("записи v3 в Arbitrum нет");
    assert!(other.contains("не проверен"), "{other}");
    let ok = live_proof::accepted_for(Version::V3, ARB, ExecKind::UniV4, false, Address::ZERO);
    assert!(!ok);
}

#[test]
fn rule_is_strict_on_altered_record() {
    let (summary, reject) = (text("summary.json"), text("hooks_reject.json"));
    let cur = code::runtime_sha256_of(Version::V3);
    let none = live_rule::judge("README без хеша", &summary, &reject, cur).unwrap();
    assert!(
        none.iter().all(|x| !x.accepted),
        "запись о другом коде — ничего не принято"
    );
    assert!(deploy_gate::refusal_for(Version::V3, BSC, &Ok(none)).is_some());
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

/// Данные вызова (нижний регистр): поле `input` (так пишет запись eth_simulateV1) или `data` (так пишет запись
/// eth_call). None — нет ни того, ни другого.
fn call_input(c: &Value) -> Option<String> {
    c["input"]
        .as_str()
        .or_else(|| c["data"].as_str())
        .map(str::to_ascii_lowercase)
}

/// Адреса квотеров DEX из README записи (Uniswap V4, Infinity CL, Infinity Bin).
const QUOTERS: [&str; 3] = [
    "0x95c086d7fe28add842ee220fef225a777474d781",
    "0xd0737c9762912dd34c3271197e362aa736df0926",
    "0xc631f4b0fc2dd68ad45f74b2942628db117dd359",
];

/// Ревью t185: остаток на исполнителе и квотер (а) — по сырым ответам узла, а не по полям summary.json.
#[test]
fn x_left_and_quoter_a_from_raw_rpc() {
    let raw = raw_rpc();
    let raw = raw.as_array().unwrap();
    let x_word = &X[2..];
    let mut executes = 0;
    for s in raw.iter().filter(|x| x["method"] == "eth_simulateV1") {
        let req = s["params"][0]["blockStateCalls"][0]["calls"]
            .as_array()
            .unwrap();
        let res = s["result"][0]["calls"].as_array().unwrap();
        for (i, c) in req.iter().enumerate() {
            if !c["to"].as_str().is_some_and(|t| t.eq_ignore_ascii_case(X)) {
                continue;
            }
            executes += 1;
            // Сразу за execute — balanceOf(X) по обоим токенам шага, оба 0.
            for j in [i + 1, i + 2] {
                // Вызовы eth_simulateV1 записи несут данные в поле `input` (execution-apis GenericTransaction).
                let data = call_input(&req[j]).unwrap_or_else(|| {
                    panic!("вызов {j} симуляции: нет input / data после execute {i}")
                });
                assert!(
                    data.starts_with("0x70a08231") && data.ends_with(x_word),
                    "balanceOf(X)"
                );
                assert_eq!(res[j]["status"], "0x1");
                let ret = res[j]["returnData"]
                    .as_str()
                    .unwrap_or_else(|| panic!("итог вызова {j}: нет returnData"));
                assert_eq!(word(ret), Some(U256::ZERO));
            }
        }
    }
    assert_eq!(executes, 43, "все вызовы execute записи");
    // (а): ответ квотера на H (eth_call) с той же суммой и тем же токеном выхода = quoter_out = получено.
    for r in json("summary.json").as_array().unwrap() {
        if r["case"] != "a" {
            continue;
        }
        let Some(q) = dec(&r["quoter_out"]) else {
            continue;
        };
        assert_eq!(dec(&r["got"]), Some(q));
        let amount =
            alloy_primitives::hex::encode(dec(&r["amount_in"]).unwrap().to_be_bytes::<32>());
        let out = r["token_out"].as_str().unwrap()[2..].to_ascii_lowercase();
        let hit = raw.iter().any(|x| {
            let to = x["params"][0]["to"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let data = call_input(&x["params"][0]).unwrap_or_default();
            x["method"] == "eth_call"
                && x["params"][1] == TAG
                && QUOTERS.contains(&to.as_str())
                && data.contains(&amount)
                && data.contains(&out)
                && x["result"].as_str().and_then(word) == Some(q)
        });
        assert!(hit, "квотер (а) на H: {}", r["pool_id"]);
    }
}

/// Вердикты по изменённой копии записи. Ошибок нет (паника — не разобралось).
fn judge_with(readme: &str, summary: &Value, reject: &Value) -> Vec<live_rule::Verdict> {
    live_rule::judge(
        readme,
        &summary.to_string(),
        &reject.to_string(),
        code::runtime_sha256_of(Version::V3),
    )
    .unwrap()
}

/// Принят ли класс в вердиктах. Ошибок нет.
fn accepted(v: &[live_rule::Verdict], c: Class) -> bool {
    v.iter().any(|x| x.class == c && x.accepted)
}

/// Изменяет строки summary.json класса Uniswap V4 без крючка (вид 19, hooks = 0) данной case. Ошибок нет.
fn alter_v4(summary: &mut Value, case: &str, f: impl Fn(&mut Value)) {
    for r in summary.as_array_mut().unwrap() {
        let zero =
            r["hooks"].as_str().and_then(|h| h.parse::<Address>().ok()) == Some(Address::ZERO);
        if r["kind"] == 19 && zero && r["case"] == case {
            f(r);
        }
    }
}

/// Ревью t185, пп. 1–4: каждое изменение копии записи — класс Uniswap V4 без крючка не принят (Infinity CL без крючка
/// при этом остаётся принятым — изменение точечное).
#[test]
fn rule_strict_review_t185() {
    let readme = text("README.md");
    let (summary, reject) = (json("summary.json"), json("hooks_reject.json"));
    let (v4, cl) = (class(19, Hook::Without), class(20, Hook::Without));
    let base = judge_with(&readme, &summary, &reject);
    assert!(accepted(&base, v4) && accepted(&base, cl));
    // 1. Откат пула без крючка «объяснён» записью hooks_reject с hooks = 0 — всё равно необъяснённое.
    let mut s1 = summary.clone();
    let mut r1 = reject.clone();
    let mut pool = String::new();
    for r in s1.as_array_mut().unwrap() {
        if pool.is_empty() && r["kind"] == 19 && r["case"] == "a" {
            pool = r["pool_id"].as_str().unwrap().to_string();
            r["status"] = 0.into();
            r["got"] = Value::Null;
            r["revert"] = "тест".into();
            r1.as_array_mut().unwrap().push(serde_json::json!({
                "pool_id": pool, "hooks": r["hooks"], "revert": "тест", "case": "a"
            }));
        }
    }
    let v = judge_with(&readme, &s1, &r1);
    assert!(
        !accepted(&v, v4) && accepted(&v, cl),
        "откат пула без крючка: {pool}"
    );
    // 1б. Крючок в hooks_reject не тот, что у пула, — откат крючкового пула необъяснённый.
    let mut r1b = reject.clone();
    r1b[0]["hooks"] = format!("{:#x}", Address::repeat_byte(0x11)).into();
    let v = judge_with(&readme, &summary, &r1b);
    let why = &v
        .iter()
        .find(|x| x.class == class(20, Hook::With))
        .unwrap()
        .why;
    assert!(why.contains("откат вне hooks_reject.json"), "{why}");
    // 2. x_left пустой или без ключа токена — пул не доказан.
    for strip in [None, Some("coin"), Some("usdt")] {
        let mut s2 = summary.clone();
        for case in ["a", "b"] {
            alter_v4(&mut s2, case, |r| match strip {
                None => r["x_left"] = serde_json::json!({}),
                Some(k) => {
                    r["x_left"].as_object_mut().unwrap().remove(k);
                }
            });
        }
        let v = judge_with(&readme, &s2, &reject);
        assert!(!accepted(&v, v4) && accepted(&v, cl), "x_left {strip:?}");
    }
    // 3. Получено = квотер = 0 — не доказательство.
    let mut s3 = summary.clone();
    alter_v4(&mut s3, "a", |r| {
        r["got"] = "0".into();
        r["quoter_out"] = "0".into();
    });
    let v = judge_with(&readme, &s3, &reject);
    assert!(!accepted(&v, v4) && accepted(&v, cl), "нули");
    // 4. SHA текущего кода — в README, но не в строке «sha256 симулированного bin-runtime:» — ничего не принято.
    let cur = code::runtime_sha256_of(Version::V3).unwrap();
    let other = format!(
        "- sha256 симулированного bin-runtime: {} (другая сборка)\nКод НЕ совпал с {cur}\n",
        "0".repeat(64)
    );
    let v = judge_with(&other, &summary, &reject);
    assert!(v.iter().all(|x| !x.accepted), "SHA не в своей строке");
    assert!(live_rule::bound(&readme, cur));
    assert!(!live_rule::bound(&format!("код НЕ совпал с {cur}"), cur));
}
