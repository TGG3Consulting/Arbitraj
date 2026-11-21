//! Файл: tests/bsc_dex_executor_v5_live.rs
//! Что делает: решение 488 (T488-02, docs/notes/arbitrum/v5-record-integration.md) — приёмка исполнителя v5 по классам шага
//! на записи симуляции живых пулов Arbitrum One (tests/data/executor_v5_live_2026-11-21, tools/arbitrum_record/exec_v5_live.py —
//! eth_simulateV1 своего узла на H = 526309916, только чтение; копия exec_v4_live.py под сборку v5). Без сети:
//! 1. Сторож: строка README «sha256 симулированного bin-runtime» несёт SHA-256 кода v5 (contracts/out/ArbExecutor.bin-runtime);
//!    строка «- chainId: 42161» = поле chain_id строк summary.json = eth_chainId узла в raw_rpc.json; в каждой симуляции код подменён
//!    только у X и побайтно = bin-runtime v5, блок — H.
//! 2. Вердикт правила (live_rule.rs) для пары (v5, 42161): приняты Uniswap V4 (вид 19) без крючка (3 из 3 пулов) и Balancer V3 (23)
//!    без крючка (2 из 2: 0x5418…2161 без крючка и 0x412d…6fc6 с StableSurge, bv3_hook_kind surge); у каждого (а) и (б) — статус 1,
//!    x_left 0 / 0, получено = кредит Swap хранилища, (а) получено = quoter_out, (б) получено = quoter_out_after_a. Виды крючка
//!    static / mev — «нет пулов»; 20 / 21 — пулов нет. После сборки deploy_gate::refusal_at(Some(42161)) для v5 = None (кнопка
//!    «Развернуть исполнитель» в блоке сети Arbitrum открыта, решение 491); в BSC (56) и без сети записи v5 нет — закрыто.
//!    Новые виды v5 (26 AERO_BASIC, 27 BALANCER_V3_BUF, 28 CURVE_ETH …) этой записью не покрыты (README записи).
//! 3. Виды 22 / 24 / 1 / 0 (правило не судит): в (б) получено = quoter_out_after_a; quoter_out (б) — квотер на H до (а) (другое
//!    состояние, не сравнивается), у LB v2.0 (24) он откатился TreeMath__ErrorDepthSearch (null).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::executor::{code, deploy_gate, live_proof, live_record, live_rule},
//! pools::exec_kind, serde_json.

use std::collections::BTreeMap;

use alloy_primitives::Address;
use serde_json::Value;

use arb::venues::bsc_dex::exec::executor::code::{self, Version};
use arb::venues::bsc_dex::exec::executor::live_rule::{self, Class, Hook};
use arb::venues::bsc_dex::exec::executor::{deploy_gate, live_proof, live_record};
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;

const DIR: &str = "tests/data/executor_v5_live_2026-11-21";
/// Блок записи H = 526309916 (README записи).
const TAG: &str = "0x1f5eda1c";
/// Адрес X исполнителя в симуляции (README записи: пустой адрес с подменой кода).
const X: &str = "0xc16f44b608bcab7f92d586a404ac10af79eaa2b6";
/// Сеть записи — Arbitrum One (README: «- chainId: 42161»).
const ARB: Option<u64> = Some(42161);
/// Сеть BSC — записи v5 в ней нет.
const BSC: Option<u64> = Some(56);

/// Текст файла записи. Ошибок нет (паника — файла нет).
fn text(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{DIR}/{name}", env!("CARGO_MANIFEST_DIR"))).expect("запись")
}

/// JSON файла записи. Ошибок нет (паника).
fn json(name: &str) -> Value {
    serde_json::from_str(&text(name)).expect("JSON")
}

/// Класс по виду и крючку (для чтения ожиданий).
fn class(kind: u8, hook: Hook) -> Class {
    Class { kind, hook }
}

/// Принят ли шаг (без крючка, не Stable) у исполнителя v5 в сети. Ошибок нет.
fn v5(chain: Option<u64>, k: ExecKind) -> bool {
    live_proof::accepted_for(Version::V5, chain, k, false, Address::ZERO)
}

#[test]
fn record_bound_to_v5_code_and_arbitrum() {
    let cur = code::runtime_sha256_of(Version::V5).expect("SHA-256 bin-runtime v5 записан");
    let readme = text("README.md");
    assert!(live_rule::bound(&readme, cur), "README записи — о коде v5");
    let chain = live_record::chain_in(&readme, &text("summary.json"));
    assert_eq!(chain, Ok(42161));
    assert!(live_record::chain_in("без строки сети", "[]").is_err());
    // Запись сделана кодом v5 — текущей сборкой contracts/out/.
    let runtime = std::fs::read_to_string(format!(
        "{}/contracts/out/ArbExecutor.bin-runtime",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let runtime = runtime.trim().trim_start_matches("0x").to_ascii_lowercase();
    let raw = json("raw_rpc.json");
    let all = raw.as_array().unwrap();
    let id = all.iter().find(|x| x["method"] == "eth_chainId");
    assert_eq!(id.expect("eth_chainId")["result"], "0xa4b1");
    let sims: Vec<&Value> = all
        .iter()
        .filter(|x| x["method"] == "eth_simulateV1")
        .collect();
    assert!(!sims.is_empty());
    for s in &sims {
        assert_eq!(s["params"][1], TAG, "симуляция на блоке H");
        let ov = s["params"][0]["blockStateCalls"][0]["stateOverrides"]
            .as_object()
            .expect("подмены");
        let mut x = 0;
        for (a, v) in ov {
            if let Some(code) = v.get("code").and_then(Value::as_str) {
                assert!(a.eq_ignore_ascii_case(X), "код подменён только у X");
                let code = code.trim_start_matches("0x").to_ascii_lowercase();
                assert_eq!(code, runtime, "в симуляции — bin-runtime v5");
                x += 1;
            }
        }
        assert_eq!(x, 1, "код X подменён в каждой симуляции");
    }
}

#[test]
fn verdict_v5_arbitrum_by_rule() {
    let v = live_proof::verdicts_of(Version::V5, ARB)
        .as_ref()
        .expect("запись разобрана");
    let got: BTreeMap<Class, (usize, usize, bool)> = v
        .iter()
        .map(|x| (x.class, (x.pools, x.proven, x.accepted)))
        .collect();
    let want: BTreeMap<Class, (usize, usize, bool)> = [
        (class(19, Hook::Without), (3, 3, true)),
        (class(19, Hook::With), (0, 0, false)),
        (class(20, Hook::Without), (0, 0, false)),
        (class(20, Hook::With), (0, 0, false)),
        (class(20, Hook::Stable), (0, 0, false)),
        (class(21, Hook::Without), (0, 0, false)),
        (class(21, Hook::With), (0, 0, false)),
        // Два пула Balancer V3 доказаны (без крючка и StableSurge) — MIN_POOLS = 2.
        (class(23, Hook::Without), (2, 2, true)),
    ]
    .into_iter()
    .collect();
    assert_eq!(got, want);
    let why = |c: Class| v.iter().find(|x| x.class == c).unwrap().why.clone();
    let w23 = why(class(23, Hook::Without));
    assert!(w23.starts_with("принят: доказано 2 из 2 пулов"), "{w23}");
    assert!(why(class(19, Hook::With)).contains("класс с крючком"));
    // Виды 22 / 24 / 1 / 0 правило не судит — их классов нет.
    assert!(v.iter().all(|x| [19, 20, 21, 23].contains(&x.class.kind)));
    // Решение 488, 491: текущая сборка — v5; запись v5 × 42161 приняла классы — развёртывание открыто в Arbitrum, в BSC и без
    // сети записи v5 нет — закрыто.
    assert_eq!(code::current_version(), Ok(Version::V5));
    assert!(
        deploy_gate::refusal_at(ARB).is_none(),
        "запись v5 × 42161 приняла хотя бы один класс"
    );
    for chain in [BSC, None] {
        let why = deploy_gate::refusal_at(chain)
            .expect("записи v5 для этой сети нет — развёртывание закрыто");
        assert!(why.contains("v5"), "{why}");
    }
    assert!(!deploy_gate::closed_everywhere());
    // Шаги: Uniswap V4 без крючка и Balancer V3 у v5 — только в Arbitrum; не шаг менеджера — всегда.
    assert!(v5(ARB, ExecKind::UniV4));
    assert!(!v5(BSC, ExecKind::UniV4));
    assert!(!v5(None, ExecKind::UniV4));
    assert!(v5(ARB, ExecKind::BalancerV3));
    assert!(!v5(BSC, ExecKind::BalancerV3));
    assert!(!v5(None, ExecKind::BalancerV3));
    assert!(v5(BSC, ExecKind::V2));
    let hook = Address::repeat_byte(0x11);
    let hooked = live_proof::admits_for(Version::V5, ARB, ExecKind::UniV4, false, hook);
    assert!(hooked.is_err());
    let list = live_proof::listing_for(Version::V5, ARB);
    assert!(list.contains(DIR), "{list}");
}

/// Виды 22 / 24 / 1 / 0: в (б) получено = quoter_out_after_a (квотер той же симуляции после (а)); quoter_out (б) —
/// квотер на H до покупки (а), другое состояние — правило его не сравнивает (live_row.rs); у LB v2.0 (24) квотер на H
/// откатился (0x10d64861 — TreeMath__ErrorDepthSearch), quoter_out = null. got_eq_swap_credit у этих видов не записан.
#[test]
fn unjudged_kinds_sell_matches_quoter_after_a() {
    let rows = json("summary.json");
    let mut n = 0;
    for r in rows.as_array().unwrap() {
        let kind = r["kind"].as_u64().unwrap();
        if r["case"] != "b" || ![0, 1, 22, 24].contains(&kind) {
            continue;
        }
        assert_eq!(r["status"], 1);
        assert_eq!(r["got"], r["quoter_out_after_a"], "{r}");
        assert!(r["got_eq_swap_credit"].is_null());
        if kind == 24 {
            assert!(r["quoter_out"].is_null());
            assert!(r["quoter_err"].as_str().unwrap().contains("0x10d64861"));
        } else {
            assert_ne!(r["got"], r["quoter_out"], "{r}");
        }
        n += 1;
    }
    assert_eq!(n, 10);
}
