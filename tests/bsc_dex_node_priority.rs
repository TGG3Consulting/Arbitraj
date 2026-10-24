//! Файл: tests/bsc_dex_node_priority.rs
//! Что делает: очередь узла со срочностью (решение 61, node/pace.rs, node/priority.rs): освободившееся место получает
//! сделка, потом проверка, потом фон — независимо от порядка прихода; отменённый ждущий не задерживает очередь; класс
//! задачи — из scope. И откат «срок транзакции вышел» узнаётся по селектору 0x5bf6f916 — он записан в живом журнале
//! Тиграна 2026-10-24 («шаг 4 пробной покупки откатился … 0x5bf6f916», GTAI, XRP).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::node::{pace, priority}, arb::venues::bsc_dex::exec::ur.
//! Шаг очереди — параметр теста (50 мс настоящего времени), не настройка системы.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy::sol_types::SolError;
use arb::venues::bsc_dex::exec::ur;
use arb::venues::bsc_dex::node::pace::Pace;
use arb::venues::bsc_dex::node::priority::{self, Class};

const STEP: Duration = Duration::from_millis(50);

/// Запускает ждущего с классом и записывает, когда он получил место.
fn waiter(pace: &Pace, class: Class, done: &Arc<Mutex<Vec<Class>>>) -> tokio::task::JoinHandle<()> {
    let (pace, done) = (pace.clone(), done.clone());
    tokio::spawn(async move {
        pace.take(class, STEP).await;
        done.lock().unwrap().push(class);
    })
}

#[tokio::test]
async fn freed_slot_goes_to_trade_then_check_then_background() {
    let pace = Pace::default();
    // Очередь занята фоновым запросом.
    pace.take(Class::Background, STEP).await;
    let done = Arc::new(Mutex::new(Vec::new()));
    // Приходят в обратном порядке срочности.
    let tasks = [
        waiter(&pace, Class::Background, &done),
        waiter(&pace, Class::Check, &done),
        waiter(&pace, Class::Trade, &done),
    ];
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(
        *done.lock().unwrap(),
        vec![Class::Trade, Class::Check, Class::Background]
    );
}

#[tokio::test]
async fn same_class_keeps_arrival_order_and_respects_pace() {
    let pace = Pace::default();
    let start = tokio::time::Instant::now();
    pace.take(Class::Check, STEP).await;
    pace.take(Class::Check, STEP).await;
    pace.take(Class::Check, STEP).await;
    // Три места по шагу: третье — не раньше чем через два шага.
    assert!(start.elapsed() >= STEP * 2);
}

#[tokio::test]
async fn cancelled_waiter_leaves_queue() {
    let pace = Pace::default();
    pace.take(Class::Background, STEP).await;
    let done = Arc::new(Mutex::new(Vec::new()));
    let trade = waiter(&pace, Class::Trade, &done);
    tokio::task::yield_now().await;
    trade.abort();
    let _ = trade.await;
    // Отменённый ждущий остался бы первым в очереди навсегда — фоновый не дождался бы места.
    let bg = waiter(&pace, Class::Background, &done);
    tokio::time::timeout(STEP * 4, bg)
        .await
        .expect("отменённый ждущий держит очередь")
        .unwrap();
    assert_eq!(*done.lock().unwrap(), vec![Class::Background]);
}

#[tokio::test]
async fn class_comes_from_scope_default_background() {
    assert_eq!(priority::current(), Class::Background);
    let inner = priority::scope(Class::Trade, async { priority::current() }).await;
    assert_eq!(inner, Class::Trade);
}

#[test]
fn deadline_revert_is_recognised_by_recorded_selector() {
    let sel = ur::IUniversalRouter::TransactionDeadlinePassed::SELECTOR;
    assert_eq!(alloy_primitives::hex::encode(sel), "5bf6f916");
    assert!(ur::deadline_passed(&sel, ""));
    // Узел может отдать данные отката только текстом ошибки.
    assert!(ur::deadline_passed(&[], "execution reverted: 0x5bf6f916"));
    assert!(!ur::deadline_passed(
        &[0x08, 0xc3, 0x79, 0xa0],
        "execution reverted"
    ));
}
