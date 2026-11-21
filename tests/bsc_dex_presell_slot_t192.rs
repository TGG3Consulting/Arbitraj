//! Файл: tests/bsc_dex_presell_slot_t192.rs
//! Что делает: решение 407 (t192) — подмена баланса монеты кошелька в симуляции продажи до покупки
//! (exec/presell_slot.rs, check/simulate.rs Override.diff) на ЖИВОЙ записи своего узла
//! tests/data/executor_v3_live_2026-11-17 (raw_rpc.json.zst, блок H = 0x7ff4ca0, только чтение). Без сети:
//! 1. Ключ Solidity keccak256(pad(holder) . pad(slot)) = ключам eth_getStorageAt записи (USDT, держатель Vault
//!    Infinity, слоты 0 и 1), и значение слота 1 = ответу balanceOf(Vault) того же блока — правило «слот верный, если
//!    balanceOf = значению слота» на живом токене; это же — проверка упаковки (ревью М5, presell_slot_find::unpacked:
//!    слово слота = balanceOf → слово — только баланс). Ключ Vyper (обратный порядок) — другой.
//! 2. Подмена баланса (credit) в теле eth_simulateV1 — stateDiff без code, ключ и значение = записанной подмене баланса
//!    W по USDT (та же запись, слот 1), у записанной подмены USDT тоже нет code.
//! 3. Запрос поиска для NXT (0xae7484d162ba80b340eba7769a7a67838b1c16c1 — адрес из записей
//!    cex_samples_2026_11_04/kucoin_currencies_v3.json и cand_rejects_2026-11-16/cases.json): одна подмена токена,
//!    2·N разных ключей (Solidity и Vyper), метки разные и больше нужного, один вызов balanceOf(holder) от holder;
//!    выбор слота — только при ровном совпадении с меткой.
//! 4. Расчёт «без потолка»: presell_sim — не «бюджет срезал», подпись есть.
//!
//! Ответа узла на сам поиск (eth_simulateV1 с метками) в записях нет — дозапись: docs/TASKS.md (t192).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{check::simulate, exec::presell_slot}, arb::core::uncapped_types,
//! arb::report::missed_labels, zstd, serde_json.

use alloy_primitives::{Address, B256, U256, address};
use serde_json::Value;

use arb::core::uncapped_types::budget_bound;
use arb::report::missed_labels::limited_label;
use arb::venues::bsc_dex::check::simulate::{self, Base};
use arb::venues::bsc_dex::exec::presell_slot::{self, Layout, Slot};
use arb::venues::bsc_dex::exec::presell_slot_find;

const DIR: &str = "tests/data/executor_v3_live_2026-11-17";
/// Блок записи H (README записи).
const H: u64 = 0x7ff4ca0;
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
/// Держатель, на котором запись искала слот баланса USDT (slots.json balance_holder_used — Vault Infinity).
const VAULT: Address = address!("0x238a358808379702088667322f80ac48bad5e6c4");
/// Кошелёк W записи (README: «Пробы и сигналы», w1).
const W: Address = address!("0x1c8f435a7a86a47490e9c9e15cd1af573985d94d");
/// NXT (сделка 20261118114214809NXT).
const NXT: Address = address!("0xae7484d162ba80b340eba7769a7a67838b1c16c1");

/// Сжатая запись raw_rpc.json.zst → массив запросов. Ошибок нет (паника).
fn raw_rpc() -> Vec<Value> {
    let p = format!("{}/{DIR}/raw_rpc.json.zst", env!("CARGO_MANIFEST_DIR"));
    let raw = zstd::decode_all(std::fs::File::open(p).expect("запись")).expect("zstd");
    serde_json::from_slice(&raw).expect("JSON")
}

/// hex-слово → U256. Ошибок нет (паника).
fn word(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

/// Ключ строкой, как в запросах узла. Ошибок нет.
fn hex(k: B256) -> String {
    format!("{k:#x}")
}

const fn sol(index: u64) -> Slot {
    Slot {
        index,
        layout: Layout::Solidity,
    }
}

#[test]
fn solidity_key_matches_recorded_storage_and_balance() {
    let raw = raw_rpc();
    let tag = format!("{H:#x}");
    let storage: Vec<&Value> = raw
        .iter()
        .filter(|e| e["method"] == "eth_getStorageAt" && e["params"][0] == format!("{USDT:#x}"))
        .collect();
    assert_eq!(storage.len(), 2, "в записи два чтения слота USDT");
    let at = |k: B256| {
        storage
            .iter()
            .find(|e| e["params"][1] == hex(k) && e["params"][2] == tag)
            .map(|e| word(&e["result"]))
    };
    assert_eq!(at(presell_slot::key(sol(0), VAULT)), Some(U256::ZERO));
    let slot1 = at(presell_slot::key(sol(1), VAULT)).expect("ключ слота 1 = ключу записи");
    let data = format!(
        "0x70a08231{}",
        hex(VAULT.into_word()).trim_start_matches("0x")
    );
    let bal = raw
        .iter()
        .find(|e| {
            e["method"] == "eth_call" && e["params"][0]["data"] == data && e["params"][1] == tag
        })
        .map(|e| word(&e["result"]))
        .expect("balanceOf(Vault) в записи");
    assert!(!bal.is_zero());
    assert_eq!(
        slot1, bal,
        "значение слота 1 = balanceOf — слот баланса USDT"
    );
    // Ревью М5: слово слота = balanceOf — слово только баланс (подмена разрешена); иначе — упаковано, подмены нет.
    assert!(presell_slot_find::unpacked(slot1, bal).is_ok());
    assert!(presell_slot_find::unpacked(slot1, bal - U256::from(1u8)).is_err());
    let vy = Slot {
        index: 1,
        layout: Layout::Vyper,
    };
    assert_ne!(
        presell_slot::key(vy, VAULT),
        presell_slot::key(sol(1), VAULT)
    );
}

#[test]
fn credit_is_state_diff_like_recorded_override() {
    let raw = raw_rpc();
    let k = hex(presell_slot::key(sol(1), W));
    let rec = raw
        .iter()
        .filter(|e| e["method"] == "eth_simulateV1")
        .filter_map(|e| {
            e["params"][0]["blockStateCalls"][0]["stateOverrides"].get(format!("{USDT:#x}"))
        })
        .find(|o| o["stateDiff"].get(&k).is_some())
        .expect("записанная подмена баланса W по USDT");
    assert!(rec.get("code").is_none() && rec.get("state").is_none());
    let value = word(&rec["stateDiff"][&k]);
    let over = presell_slot::credit(USDT, W, sol(1), value);
    let body = simulate::params(&[], &[], &[over], Base::from(H));
    let ours = &body[0]["blockStateCalls"][0]["stateOverrides"][format!("{USDT:#x}")];
    assert!(
        ours.get("code").is_none() && ours.get("state").is_none(),
        "{ours}"
    );
    let diff = ours["stateDiff"].as_object().unwrap();
    assert_eq!(diff.len(), 1);
    assert_eq!(word(&diff[&k]), value);
    assert_eq!(diff[&k].as_str().unwrap().len(), 66, "значение — 32 байта");
}

#[test]
fn nxt_scan_request_and_pick() {
    let n = 64;
    // Нужное количество — любое; здесь число из записи (баланс W по USDT на H, slots.json).
    let need = U256::from(1_167_036_431_137_515_051_640u128);
    let (calls, over, slots) = presell_slot::scan_request(NXT, W, need, n).expect("запрос");
    assert_eq!(slots.len(), 2 * n as usize);
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].from, calls[0].to), (W, NXT));
    assert_eq!(&calls[0].input[..4], &[0x70, 0xa0, 0x82, 0x31], "balanceOf");
    let body = simulate::params(&calls, &[], &[over], Base::from(H));
    let ov = body[0]["blockStateCalls"][0]["stateOverrides"]
        .as_object()
        .unwrap();
    assert_eq!(ov.len(), 1);
    let diff = ov[&format!("{NXT:#x}")]["stateDiff"].as_object().unwrap();
    assert_eq!(diff.len(), 2 * n as usize, "все ключи разные");
    assert!(diff.contains_key(&hex(presell_slot::key(sol(1), W))));
    let mut marks: Vec<U256> = diff.values().map(word).collect();
    marks.sort();
    marks.dedup();
    assert_eq!(marks.len(), 2 * n as usize, "все метки разные");
    assert!(marks.iter().all(|m| *m > need));
    // Ровная метка кандидата j — его слот; баланс кошелька (не метка) или чужое число — слота нет.
    let j = 3;
    let got = presell_slot::marker(need, j).unwrap();
    assert_eq!(presell_slot::pick(&slots, need, got), Some(slots[j]));
    assert_eq!(presell_slot::pick(&slots, need, U256::ZERO), None);
    assert_eq!(presell_slot::pick(&slots, need, need), None);
    // Метки не помещаются в uint256 — поиска нет.
    assert!(presell_slot::scan_request(NXT, W, U256::MAX, n).is_none());
}

#[test]
fn presell_cut_is_not_budget() {
    assert!(!budget_bound("presell_sim"));
    assert!(budget_bound("per_trade"));
    assert!(limited_label("presell_sim").contains("симуляция продажи"));
}
