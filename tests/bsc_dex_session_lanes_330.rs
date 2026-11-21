//! Файл: tests/bsc_dex_session_lanes_330.rs
//! Что делает: решение 330 — расписание полос событий пулов сессии пачки (pools/lanes.rs) на РЕАЛЬНОМ потоке
//! логов BSC: квитанции четырёх блоков подряд 129702236–129702239 (tests/data/receipts_2026-10-25/responses.json,
//! eth_getBlockReceipts публичного узла bsc-dataseed.bnbchain.org, запись техлида 2026-10-25) — 2305 логов от
//! 346 адресов в порядке узла (blockNumber, logIndex). Ключ полосы — адрес лога (как пул пачки по адресу).
//! Проверяется:
//! - события каждого ключа выполняются строго в порядке прихода, каждое ровно один раз, ни одно не теряется;
//! - у ключа одновременно не больше одного выполняющегося; всего — не больше предела; предел 0 — как 1;
//! - «медленный» пул (его первое событие не завершается, пока не обработаны все остальные) не держит события других
//!   ключей при пределе ≥ 2, а его собственные события ждут в его полосе и идут после — по порядку;
//! - при пределе 1 работа одна за другой (как раньше в сессии), всё обработано;
//! - каталог: настройки решений 330 и 331 — целые, необязательные, только площадка DEX.
//!
//! Чего здесь нет (нужна запись с живого бота, docs/TASKS.md t153): время перечитывания по событию с полосами и без,
//! одновременное чтение пулов маршрута узлом (ответы eth_call видов «по одному» на одном блоке).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::lanes, arb::settings::{catalog, schema}, serde_json.
//! Настройки: нет.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use arb::settings::catalog;
use arb::settings::schema::{Kind, VenueFor};
use arb::venues::bsc_dex::pools::lanes::Lanes;

const RECEIPTS: &str = "tests/data/receipts_2026-10-25/responses.json";

/// Позиция лога у узла: (blockNumber, logIndex).
type Pos = (u64, u64);

/// Логи блоков окна записи по порядку узла: (номер адреса, позиция). Блок создания пула (не из окна) не берётся.
fn logs() -> (Vec<(usize, Pos)>, usize) {
    let text = std::fs::read_to_string(RECEIPTS).expect("запись квитанций");
    let v: Value = serde_json::from_str(&text).expect("JSON записи");
    let mut addrs: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for r in v["results"].as_array().expect("results") {
        let name = r["name"].as_str().unwrap_or_default();
        if !name.starts_with("receipts ") || name.contains("создание") {
            continue;
        }
        for rc in r["response"]["result"].as_array().expect("квитанции") {
            for l in rc["logs"].as_array().expect("логи") {
                let hex = |k: &str| {
                    u64::from_str_radix(l[k].as_str().expect(k).trim_start_matches("0x"), 16)
                        .expect(k)
                };
                let a = l["address"].as_str().expect("address").to_string();
                let n = addrs.len();
                let key = *addrs.entry(a).or_insert(n);
                out.push((key, (hex("blockNumber"), hex("logIndex"))));
            }
        }
    }
    out.sort_by_key(|(_, p)| *p);
    (out, addrs.len())
}

/// Прогон расписания: приход логов по одному, после каждого — запуск в пределах; завершение выбирает `pick` из
/// выполняющихся (None — пока не завершать). Возвращает: порядок выполнения по ключам и наибольшее число одновременных.
fn run(
    max: usize,
    logs: &[(usize, Pos)],
    mut pick: impl FnMut(usize, &[(usize, Pos)]) -> Option<usize>,
) -> (HashMap<usize, Vec<Pos>>, usize) {
    let mut lanes: Lanes<usize> = Lanes::new();
    let mut queues: HashMap<usize, VecDeque<Pos>> = HashMap::new();
    let mut running: Vec<(usize, Pos)> = Vec::new();
    let mut done: HashMap<usize, Vec<Pos>> = HashMap::new();
    let mut peak = 0;
    let start = |lanes: &mut Lanes<usize>,
                 queues: &mut HashMap<usize, VecDeque<Pos>>,
                 running: &mut Vec<(usize, Pos)>| {
        while let Some(k) = lanes.next(max) {
            assert!(
                !running.iter().any(|(r, _)| *r == k),
                "ключ {k} выполняется дважды"
            );
            let p = queues
                .get_mut(&k)
                .and_then(|q| q.pop_front())
                .expect("работа ключа");
            running.push((k, p));
        }
    };
    for (n, &(k, p)) in logs.iter().enumerate() {
        queues.entry(k).or_default().push_back(p);
        lanes.push(k);
        start(&mut lanes, &mut queues, &mut running);
        peak = peak.max(running.len());
        assert!(running.len() <= max.max(1), "больше предела");
        assert_eq!(lanes.running(), running.len());
        if let Some(i) = pick(n, running.as_slice()) {
            let (k, p) = running.remove(i);
            done.entry(k).or_default().push(p);
            lanes.done(k);
            start(&mut lanes, &mut queues, &mut running);
        }
    }
    // Остаток — завершать по одному (первый выполняющийся).
    while !running.is_empty() {
        let (k, p) = running.remove(0);
        done.entry(k).or_default().push(p);
        lanes.done(k);
        start(&mut lanes, &mut queues, &mut running);
        assert!(running.len() <= max.max(1));
    }
    assert!(lanes.idle(), "после всего полосы пусты");
    (done, peak)
}

/// Порядок прихода по ключам.
fn arrivals(logs: &[(usize, Pos)]) -> HashMap<usize, Vec<Pos>> {
    let mut m: HashMap<usize, Vec<Pos>> = HashMap::new();
    for &(k, p) in logs {
        m.entry(k).or_default().push(p);
    }
    m
}

#[test]
fn recorded_logs_keep_order_per_pool_and_limit() {
    let (logs, keys) = logs();
    assert_eq!(logs.len(), 2305, "логи квитанций окна 129702236–129702239");
    assert_eq!(keys, 346, "разных адресов в окне");
    let want = arrivals(&logs);
    for max in [0usize, 1, 2, 4, 16] {
        // Завершается то один, то другой выполняющийся — порядок завершений разных ключей произвольный.
        let (done, peak) = run(max, &logs, |n, r| {
            if !r.is_empty() && n % 3 != 1 {
                Some(n % r.len())
            } else {
                None
            }
        });
        assert_eq!(
            done, want,
            "предел {max}: каждое событие ровно раз и по порядку своего ключа"
        );
        assert!(peak <= max.max(1));
        if max >= 4 {
            assert!(peak >= 2, "предел {max}: разные ключи шли одновременно");
        }
    }
}

#[test]
fn slow_pool_does_not_hold_other_pools() {
    let (logs, _) = logs();
    let want = arrivals(&logs);
    // Медленный пул — ключ с наибольшим числом событий в окне.
    let slow = want
        .iter()
        .max_by_key(|(_, v)| v.len())
        .map(|(k, _)| *k)
        .expect("ключ");
    let slow_n = want[&slow].len();
    assert!(slow_n >= 2, "у медленного ключа несколько событий");
    let mut others_done_while_slow = 0usize;
    let (done, _) = run(4, &logs, |_, r| {
        // Медленный не завершается, пока идёт приход; остальные — сразу.
        let i = r.iter().position(|(k, _)| *k != slow)?;
        others_done_while_slow += usize::from(r.iter().any(|(k, _)| *k == slow));
        Some(i)
    });
    assert_eq!(
        done, want,
        "все события по порядку своего ключа, медленного — тоже"
    );
    assert!(
        others_done_while_slow > 0,
        "пока медленный пул занят, события других пулов обработаны"
    );
}

#[test]
fn catalog_has_t153_settings() {
    for name in [
        "dex.event_lanes_max",
        "dex.block_read_parallel_max",
        "verify.read_parallel_max",
    ] {
        let def = catalog::find(name).unwrap_or_else(|| panic!("{name} в каталоге"));
        assert!(def.optional, "{name}: необязательная");
        assert_eq!(def.kind, Kind::Integer, "{name}: целое");
        assert!(
            matches!(def.venue_for, VenueFor::Dex),
            "{name}: только площадка DEX"
        );
    }
}
