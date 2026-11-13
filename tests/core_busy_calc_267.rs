//! Файл: tests/core_busy_calc_267.rs
//! Что проверяет: решение 267 — замер «что было выгодно, пока монета занята сделкой» (window.busy_calc).
//! 1) Сводка теневых расчётов (core/busy_sum.rs) на расчётах по записанным CAKE (asks BingX, пул V2 — tests/size_common):
//!    счётчики, максимум прибыли и вложение при нём, наибольшее прибыльное вложение, отброшенные, текст строки журнала
//!    (сравнение со сделкой, «прибыльных расчётов не было»); сводка в окне монеты (window.rs note_busy) растёт на
//!    месте; сводка окна без поля busy (старые окна) читается, окно без замера поле не пишет.
//! 2) Настоящее ядро на SQLite во временной папке (реестр шлюза пуст — расчёт даёт отказ ядра, моков нет):
//!    выключено / не задано — при занятой монете enqueue ничего не ставит и не пишет (как до решения 267);
//!    включено без интервала — замер не работает, одна запись в журнал;
//!    включено — теневой расчёт не чаще интервала, исполнитель не вызывается, обычных следов кандидата (очередь,
//!    строка «Кандидатов», журнал отказов, сделки, окна) нет, сводка — в окне монеты; монета освободилась — одна
//!    строка журнала core.busy_calc; обычный кандидат свободной монеты считается как раньше;
//!    места расчёта: предел занят настоящими — теневой отбрасывается; настоящий занял место — теневой снимается.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{busy_calc, busy_sum, candidates, window, deal}, arb::settings, arb::storage,
//! tests/size_common.

mod size_common;

use std::time::Duration;

use arb::core::Core;
use arb::core::busy_calc::{KIND, Shadows};
use arb::core::busy_sum::{BusyBest, BusyDeal, BusySum, Outcome};
use arb::core::candidates;
use arb::core::coin_key::CoinKey;
use arb::core::costs::Calc;
use arb::core::deal::Deal;
use arb::core::deps::Deps;
use arb::core::engine::lk;
use arb::core::types::{Direction, VenueId};
use arb::core::window::{WindowInfo, Windows};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Entry, Journal};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use size_common::calc::{calc, direction};
use size_common::{Rec, d};
use tokio::sync::broadcast;

/// Числа расчёта для сводки — как их берёт busy_calc.rs.
fn best(c: &Calc) -> Outcome {
    Outcome::Accept(Box::new(BusyBest {
        profit_usd: c.profit_usd,
        invested: c.invested,
        qty: c.qty,
        at: c.at,
        budget: None,
        limits: None,
    }))
}

#[tokio::test]
async fn summary_counts_and_keeps_maximum() {
    let rec = Rec::load("busy-sum").await;
    let (small, big) = (calc(&rec, d("10")), calc(&rec, d("50")));
    assert!(big.invested > small.invested);
    let dir = direction();
    let t0 = Utc::now();
    let mut s = BusySum::new(dir.clone(), t0);
    s.note(Outcome::Reject("нет бюджета: …".into()), t0);
    assert!(s.text().contains("теневых расчётов 1, прошло пороги 0"));
    assert!(
        s.text()
            .contains("прибыльных расчётов не было; последний отказ: нет бюджета")
    );
    s.note(Outcome::Dropped, t0);
    s.note(best(&small), t0);
    s.note(best(&big), t0);
    let t1 = t0 + chrono::Duration::seconds(3);
    s.note(best(&small), t1);
    assert_eq!((s.calcs, s.accepted, s.dropped), (4, 3, 1));
    assert_eq!((s.first_at, s.last_at), (t0, t1));
    assert_eq!(s.max_invested, Some(big.invested));
    // Максимум — расчёт с наибольшей прибылью (какой из двух — по самим записанным расчётам).
    let top = if big.profit_usd > small.profit_usd {
        &big
    } else {
        &small
    };
    let b = s.best.clone().unwrap();
    assert_eq!(
        (b.profit_usd, b.invested, b.qty),
        (top.profit_usd, top.invested, top.qty)
    );
    // Сделка для сравнения: до покупки (вложено 0) — вложение плана.
    s.deal = Some(BusyDeal {
        id: "d7".into(),
        direction: dir.clone(),
        invested: Decimal::ZERO,
        plan_invested: Some(top.invested / d("4")),
        plan_profit_usd: d("0.3"),
    });
    let text = s.text();
    assert!(
        text.starts_with(
            "CAKE bingx → bsc_dex: пока монета была занята сделкой d7 (вложено 0 USDT"
        ),
        "{text}"
    );
    assert!(
        text.contains("теневых расчётов 4, прошло пороги 3, отброшено без расчёта"),
        "{text}"
    );
    assert!(text.contains("(в 4 раз больше сделки)"), "{text}");
    assert!(text.contains("наибольшее прибыльное вложение"), "{text}");
    // Сводка — в открытое окно монеты: растёт на месте, другое начало — своя запись; окна нет — не пишется.
    let mut w = Windows::default();
    assert!(!w.note_busy(&s));
    let venue = [VenueId("bingx".into())];
    w.pass(&dir, "w1".into(), false, Decimal::ONE, &venue, t0);
    assert!(w.note_busy(&s));
    s.note(best(&small), t1);
    assert!(w.note_busy(&s));
    let mut next = BusySum::new(dir.clone(), t1);
    next.note(Outcome::Dropped, t1);
    assert!(w.note_busy(&next));
    let key = CoinKey::of(&dir.asset);
    let info = w.get(&key).unwrap();
    assert_eq!(info.busy, vec![s.clone(), next]);
    // Туда и обратно; сводка окна без поля (до решения 267) — пусто; окно без замера поле не пишет.
    let closed = w.close(&key, "тест", t1).unwrap();
    let back: WindowInfo = serde_json::from_str(&serde_json::to_string(&closed).unwrap()).unwrap();
    assert_eq!(back, closed);
    let mut old = serde_json::to_value(&closed).unwrap();
    assert!(old.as_object_mut().unwrap().remove("busy").is_some());
    let old: WindowInfo = serde_json::from_value(old).unwrap();
    assert!(old.busy.is_empty());
    w.pass(&dir, "w2".into(), false, Decimal::ONE, &venue, t0);
    let clean = serde_json::to_value(w.get(&key).unwrap()).unwrap();
    assert!(clean.get("busy").is_none());
}

/// Ядро на базе во временной папке (реестр шлюза пуст).
async fn start(name: &str) -> (Core, Store, Journal) {
    let dir = std::env::temp_dir().join(format!("arb-busy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: dir,
    })
    .await;
    (core, settings, journal)
}

/// Задаёт настройку всей системы.
async fn set(s: &Store, name: &str, v: serde_json::Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Записи журнала вида kind.
async fn logged(j: &Journal, kind: &str) -> Vec<Entry> {
    let all = j.list(1000, None).await.unwrap();
    all.into_iter().filter(|e| e.kind == kind).collect()
}

/// Сводка направления в памяти замера.
fn sum(core: &Core, dir: &Direction) -> Option<BusySum> {
    lk(&core.inner.windows)
        .get(&CoinKey::of(&dir.asset))
        .and_then(|w| w.busy.last().cloned())
}

/// Ждёт условие (не дольше 10 с).
async fn until(mut ok: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("условие не наступило за 10 с");
}

/// Следы обычного кандидата: очередь, строка «Кандидатов», журнал отказов кандидата и исполнителя.
async fn no_candidate_traces(core: &Core, j: &Journal) {
    assert!(lk(&core.inner.queued).is_empty());
    assert!(lk(&core.inner.candidates).is_empty());
    assert!(logged(j, "core.candidate_rejected").await.is_empty());
    assert!(logged(j, "core.start_refused").await.is_empty());
}

#[tokio::test]
async fn off_or_unset_does_nothing_for_busy_coin() {
    let (core, s, j) = start("off").await;
    let dir = direction();
    let key = CoinKey::of(&dir.asset);
    let venue = [VenueId("bingx".into())];
    lk(&core.inner.windows).pass(&dir, "w".into(), false, Decimal::ONE, &venue, Utc::now());
    lk(&core.inner.active).insert(key.clone());
    // Не задана; затем «нет» (интервал задан — он один замер не включает).
    candidates::enqueue(&core, &dir);
    set(&s, "window.busy_calc", json!(false)).await;
    set(&s, "window.busy_calc_min_ms", json!(50)).await;
    candidates::enqueue(&core, &dir);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sum(&core, &dir).is_none());
    assert!(logged(&j, KIND).await.is_empty());
    no_candidate_traces(&core, &j).await;
    lk(&core.inner.active).remove(&key);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(logged(&j, KIND).await.is_empty());
}

#[tokio::test]
async fn on_without_interval_does_not_work_and_says_once() {
    let (core, s, j) = start("unset").await;
    let dir = direction();
    lk(&core.inner.active).insert(CoinKey::of(&dir.asset));
    set(&s, "window.busy_calc", json!(true)).await;
    candidates::enqueue(&core, &dir);
    candidates::enqueue(&core, &dir);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let log = logged(&j, KIND).await;
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].message.contains("не работает"), "{}", log[0].message);
    assert!(log[0].message.contains("window.busy_calc_min_ms"));
    no_candidate_traces(&core, &j).await;
}

#[tokio::test]
async fn on_shadow_calc_only_observes() {
    let (core, s, j) = start("on").await;
    let dir = direction();
    let key = CoinKey::of(&dir.asset);
    set(&s, "window.busy_calc", json!(true)).await;
    set(&s, "window.busy_calc_min_ms", json!(400)).await;
    // Монета занята нашей сделкой (учёт ядра) и её окно открыто.
    let plan = json!({ "invested": "25" });
    let deal = Deal::new("d1".into(), dir.clone(), plan, d("0.5"));
    lk(&core.inner.deals).insert(deal.id.clone(), deal);
    lk(&core.inner.active).insert(key.clone());
    let venue = [VenueId("bingx".into())];
    lk(&core.inner.windows).pass(&dir, "w".into(), false, Decimal::ONE, &venue, Utc::now());
    for _ in 0..3 {
        candidates::enqueue(&core, &dir);
    }
    until(|| sum(&core, &dir).is_some_and(|x| x.calcs == 1)).await;
    // Внутри интервала новый тик расчёт не ставит.
    candidates::enqueue(&core, &dir);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(sum(&core, &dir).unwrap().calcs, 1);
    tokio::time::sleep(Duration::from_millis(350)).await;
    candidates::enqueue(&core, &dir);
    until(|| sum(&core, &dir).is_some_and(|x| x.calcs == 2)).await;
    let x = sum(&core, &dir).unwrap();
    // Реестр шлюза пуст — расчёт ядра отказывает: прошедших нет, причина записана; сделка для сравнения — из учёта.
    assert_eq!((x.accepted, x.dropped), (0, 0));
    assert!(x.last_reject.is_some() && x.best.is_none());
    let seen = x.deal.clone().unwrap();
    assert_eq!(
        (seen.id.as_str(), seen.plan_invested),
        ("d1", Some(d("25")))
    );
    assert_eq!(seen.plan_profit_usd, d("0.5"));
    // Только наблюдение: следов кандидата нет, сделка и замок — как были, окно не тронуто как «прошедший».
    no_candidate_traces(&core, &j).await;
    assert_eq!(core.deals().len(), 1);
    assert!(lk(&core.inner.active).contains(&key));
    let w = lk(&core.inner.windows).get(&key).unwrap();
    assert_eq!((w.directions.len(), w.max_profit), (1, Decimal::ONE));
    assert!(w.rejects.is_empty() && w.deals.is_empty());
    assert!(
        logged(&j, KIND).await.is_empty(),
        "до конца занятости строки нет"
    );
    // Монета освободилась — одна строка журнала на направление.
    lk(&core.inner.active).remove(&key);
    for _ in 0..1000 {
        if !logged(&j, KIND).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let log = logged(&j, KIND).await;
    assert_eq!(log.len(), 1, "{log:?}");
    let text = &log[0].message;
    assert!(
        text.starts_with("CAKE bingx → bsc_dex: пока монета была занята сделкой d1 "),
        "{text}"
    );
    assert!(
        text.contains("теневых расчётов 2, прошло пороги 0"),
        "{text}"
    );
    assert!(text.contains("прибыльных расчётов не было"), "{text}");
}

/// Стек потока для расчёта обычного кандидата в отладочной сборке: вложенные будущие calc_one (urgency → calc_meter →
/// calc_timing → profit::decide) без оптимизации не помещаются в стек тестового потока по умолчанию (переполнение —
/// в задаче candidates::run, по обратной трассе gdb); параметр теста, не настройка бота.
const CALC_STACK: usize = 64 * 1024 * 1024;

#[test]
fn free_coin_candidate_goes_as_before() {
    // Обычный кандидат свободной монеты считается той же задачей очереди, как раньше, — и при включённом замере.
    let body = async {
        let (core, s, j) = start("free").await;
        let dir = direction();
        set(&s, "window.busy_calc", json!(true)).await;
        set(&s, "window.busy_calc_min_ms", json!(400)).await;
        candidates::enqueue(&core, &dir);
        until(|| lk(&core.inner.candidates).contains_key(&dir)).await;
        assert!(lk(&core.inner.queued).is_empty());
        assert!(logged(&j, KIND).await.is_empty());
    };
    let run = move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(body);
    };
    let t = std::thread::Builder::new().stack_size(CALC_STACK);
    t.spawn(run).unwrap().join().unwrap();
}

#[tokio::test]
async fn shadow_takes_slot_only_when_free_and_gives_way() {
    let (core, s, _j) = start("slots").await;
    let dir = direction();
    let key = CoinKey::of(&dir.asset);
    set(&s, "window.busy_calc", json!(true)).await;
    set(&s, "window.busy_calc_min_ms", json!(60000)).await;
    set(&s, "profit.calc_concurrency", json!(1)).await;
    lk(&core.inner.active).insert(key.clone());
    let venue = [VenueId("bingx".into())];
    lk(&core.inner.windows).pass(&dir, "w".into(), false, Decimal::ONE, &venue, Utc::now());
    // Очередь теневых уже забрана задачей ядра — здесь проверяется только раздача мест.
    let mut sh = Shadows::take(&core);
    // Предел занят настоящим кандидатом — теневой отбрасывается сразу.
    sh.start(&core, dir.clone(), 1);
    let x = sum(&core, &dir).unwrap();
    assert_eq!((x.calcs, x.dropped), (0, 1));
    // Место свободно — теневой запущен; настоящий занял место — теневой снят (до первого шага расчёта).
    sh.start(&core, dir.clone(), 0);
    sh.give_way(Some(1), 1);
    until(|| sum(&core, &dir).is_some_and(|x| x.dropped == 2)).await;
    assert_eq!(sum(&core, &dir).unwrap().calcs, 0);
    // Место свободно и настоящих нет — расчёт доходит до итога; без предела настоящий теневых не снимает.
    sh.start(&core, dir.clone(), 0);
    sh.give_way(None, 5);
    until(|| sum(&core, &dir).is_some_and(|x| x.calcs == 1)).await;
    assert_eq!(sum(&core, &dir).unwrap().dropped, 2);
}
