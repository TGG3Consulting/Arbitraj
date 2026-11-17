//! Файл: tests/exp_stack_t175.rs
//! Что делает: ЭКСПЕРИМЕНТ t175 (только ветки exp/stack-*, в основную не идёт) — замер, хватает ли рабочему потоку
//! tokio стека STACK_KIB КиБ на цепочку аварии f9634504 (сделка 20261117143812842GENIUS): продавать невыгодно →
//! режим «Перегон» (stuck.mode = relay; целей перегона на стенде нет → продажа здесь по порогу) → ордер биржи не
//! исполнился (OrderNotFilled) × (RETRIES + 1) → повторы исчерпаны → продажа здесь (решение 229) → алерт.
//! Рантайм — как в проде (src/main.rs: #[tokio::main] — многопоточный, enable_all), но размер стека рабочих потоков —
//! из окружения STACK_KIB; stuck.unsent_retries_max — из окружения RETRIES (параметры эксперимента, не настройки бота).
//! Подготовка ядра (Core::start, настройки) — на потоке теста через block_on; сама цепочка — tokio::spawn на рабочем
//! потоке рантайма. Успех — строка «OK stack=… retries=… orders=…»; переполнение — процесс падает с «has overflowed
//! its stack» (сам тест его не ловит — ловит workflow .github/workflows/exp-stack.yml).
//! Стенд — tests/t175_common (XT по HTTP к локальному серверу; ответ на ордер — по документации XT, не записан).
//! Тест помечен #[ignore]: без переменных окружения не запускается (обычные прогоны его пропускают).
//! Кто вызывает: cargo test --release --test exp_stack_t175 -- --ignored --nocapture (workflow exp-stack.yml).
//! От чего зависит: tests/t175_common, arb::core::sell (sell_or_stuck — публичная в обеих сборках).

#[allow(dead_code)]
mod t175_common;

use arb::core::deal::Step;
use serde_json::json;
use t175_common::{ID, deal, dir, orders, saved, set, start};

/// Целое из переменной окружения. Нет или не число — паника с именем переменной.
fn env_u64(name: &str) -> u64 {
    let v = std::env::var(name).unwrap_or_else(|_| panic!("нет переменной окружения {name}"));
    v.trim()
        .parse()
        .unwrap_or_else(|_| panic!("{name} = «{v}» — не целое"))
}

#[test]
#[ignore]
fn chain_on_worker() {
    let kib = env_u64("STACK_KIB");
    let retries = env_u64("RETRIES");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size((kib * 1024) as usize)
        .build()
        .unwrap();
    let (n, step) = rt.block_on(async move {
        let d = dir(&format!("exp-stack-{kib}-{retries}"));
        let (core, s, _journal, srv) = start(&d).await;
        set(&s, "stuck.mode", json!("relay")).await;
        set(&s, "stuck.max_relays", json!(1)).await;
        set(&s, "stuck.relay_max_loss_pct", json!("5")).await;
        set(&s, "stuck.unsent_retries_max", json!(retries)).await;
        let c = core.clone();
        // Цепочка — на рабочем потоке рантайма (стек STACK_KIB), как задача сделки в проде (executor.rs: tokio::spawn).
        let h = tokio::spawn(async move {
            arb::core::sell::sell_or_stuck(&c, deal(ID, "200")).await;
        });
        h.await.unwrap();
        (orders(&srv), saved(&core, ID).step)
    });
    rt.shutdown_background();
    assert_eq!(step, Step::WaitingHuman, "итог цепочки — алерт");
    // Ордера: 1 + повторы + 1 продажа здесь; при 0 повторов повтора нет — один ордер и алерт.
    let want = if retries == 0 {
        1
    } else {
        retries as usize + 2
    };
    assert_eq!(n, want, "число ордеров");
    println!("OK stack={kib} retries={retries} orders={n}");
}
