//! Файл: tests/core_chain_run_432.rs
//! Что делает: решение 432 (шаг 4 двух сетей) — запуск и остановка торговли по сети (core/chain_run.rs) на состоянии
//! риска и записанном направлении живого бота: (а) одна сеть — кнопок по сети нет, команда сети — отказ, отказа входу
//! нет; (б) «Старт сети» из «Стоп» — запущена только она; (в) «Старт» второй сети при «В работе» — обе; (г) «Стоп»
//! последней запущенной сети — общий «Стоп»; (д) сеть записанного направления (без сети = сеть по умолчанию, настоящая
//! SQLite настроек) остановлена — отказ входу с причиной; (е) сеть не прочитать при остановленной сети — отказ.
//! Записи: tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl (поле direction — как в учёте бота).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{chain, chain_run, risk, types}, arb::settings, arb::storage. Моков нет.

use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::chain;
use arb::core::chain_run::{apply, refusal, view};
use arb::core::risk::{Mode, Risk};
use arb::core::types::{ChainId, Direction, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

const DEALS: &str = "tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl";

/// Первое направление сделки из выгрузки живого бота. Паника — файла нет.
fn recorded() -> Direction {
    let raw = std::fs::read_to_string(DEALS).unwrap();
    let line = raw.lines().find(|l| !l.trim().is_empty()).unwrap();
    serde_json::from_value(serde_json::from_str::<Value>(line).unwrap()["direction"].clone())
        .unwrap()
}

/// Сеть по id. Ошибок нет.
fn c(id: &str) -> ChainId {
    ChainId(id.into())
}

/// Две сети площадок DEX. Ошибок нет.
fn two() -> Vec<ChainId> {
    vec![c("bsc"), c("arbitrum")]
}

#[test]
fn one_chain_has_no_chain_buttons() {
    let mut r = Risk::default();
    let one = vec![c("bsc")];
    assert!(apply(&mut r, &one, &c("bsc"), true, "тест").is_err());
    assert!(apply(&mut r, &[], &c("bsc"), false, "тест").is_err());
    assert_eq!(r.mode, Mode::Stopped, "режим не тронут");
    assert!(r.chain_off.is_empty());
    assert!(view(&r, &[]).is_empty(), "одна сеть — строк нет");
}

#[test]
fn start_one_chain_from_stop_runs_only_it() {
    let mut r = Risk::default();
    apply(&mut r, &two(), &c("bsc"), true, "тест").unwrap();
    assert_eq!(r.mode, Mode::Running);
    assert!(!r.chain_off.contains_key("bsc"));
    assert!(r.chain_off.contains_key("arbitrum"));
    let v = view(&r, &two());
    assert!(v[0].running && !v[1].running, "{v:?}");
    assert!(refusal(&r, Ok(c("bsc"))).is_none());
    let why = refusal(&r, Ok(c("arbitrum"))).unwrap();
    assert!(why.contains("сеть arbitrum остановлена"), "{why}");
}

#[test]
fn second_start_adds_and_last_stop_stops_all() {
    let mut r = Risk::default();
    apply(&mut r, &two(), &c("bsc"), true, "тест").unwrap();
    apply(&mut r, &two(), &c("arbitrum"), true, "тест").unwrap();
    assert!(r.chain_off.is_empty(), "обе сети в работе");
    apply(&mut r, &two(), &c("bsc"), false, "тест").unwrap();
    assert_eq!(r.mode, Mode::Running, "arbitrum ещё торгует");
    apply(&mut r, &two(), &c("arbitrum"), false, "тест").unwrap();
    assert_eq!(r.mode, Mode::Stopped, "запущенных сетей нет — общий «Стоп»");
    assert!(view(&r, &two()).iter().all(|x| !x.running));
    assert!(apply(&mut r, &two(), &c("solana"), true, "тест").is_err());
}

#[tokio::test]
async fn recorded_direction_of_stopped_chain_is_refused() {
    let dir = std::env::temp_dir().join(format!("arb-chain-run-432-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    let s = Store::load(db.clone(), tx, Journal::new(db, jtx))
        .await
        .unwrap();
    for (v, id) in [("bsc_dex", "bsc"), ("arb_dex", "arbitrum")] {
        s.set("dex.chain", Scope::Venue(v.into()), Some(json!(id)), "тест")
            .await
            .unwrap();
    }
    let dex = [VenueId("bsc_dex".into()), VenueId("arb_dex".into())];
    let d = recorded();
    let mut r = Risk::default();
    apply(&mut r, &two(), &c("arbitrum"), true, "тест").unwrap();
    // Сеть по умолчанию не задана — сеть монеты без сети не прочитать: при остановленной сети — отказ, не догадка.
    let why = refusal(&r, chain::of(&s, &dex, &d.asset)).unwrap();
    assert!(why.contains("не прочитать"), "{why}");
    s.set(
        "dex.default_chain",
        Scope::Global,
        Some(json!("bsc")),
        "тест",
    )
    .await
    .unwrap();
    let why = refusal(&r, chain::of(&s, &dex, &d.asset)).unwrap();
    assert!(why.contains("сеть bsc остановлена"), "{why}");
    apply(&mut r, &two(), &c("bsc"), true, "тест").unwrap();
    assert!(refusal(&r, chain::of(&s, &dex, &d.asset)).is_none());
}

/// Проверяет 16 состояний четырёх сетей и каждый отдельный переход из каждого состояния.
#[test]
fn independent_control_for_bsc_arbitrum_base_and_ethereum() {
    let all = [c("bsc"), c("arbitrum"), c("base"), c("ethereum")];
    for mask in 0..16_u8 {
        for changed in 0..all.len() {
            let mut risk = Risk::default();
            for (i, chain) in all.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0) {
                apply(&mut risk, &all, chain, true, "оператор").unwrap();
                assert!(view(&risk, &all)[i].running);
            }
            let mask_after = mask ^ (1 << changed);
            apply(
                &mut risk,
                &all,
                &all[changed],
                mask_after & (1 << changed) != 0,
                "оператор",
            )
            .unwrap();
            for (i, chain) in view(&risk, &all).iter().enumerate() {
                assert_eq!(chain.running, mask_after & (1 << i) != 0);
            }
            assert_eq!(risk.mode == Mode::Running, mask_after != 0);
        }
    }
}
