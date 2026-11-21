//! Файл: tests/settings_coalesce_279.rs
//! Что проверяет: решение 279 — склейка изменений настроек (settings/live.rs Watch::next_batch), на которой стоят
//! тяжёлые проходы DEX (pools/factory_scan.rs, pools/index_watch.rs, pools/created.rs, directory/refresh.rs,
//! wallet/events.rs): шаблон «проход → next_batch → проход».
//! 1) N изменений в очереди → ОДНО пробуждение; проход читает итоговое значение из хранилища (последнее), а не
//!    снимок первого изменения; посторонние имена (не по префиксу) пробуждения не дают;
//! 2) одно изменение → одно пробуждение («1 в 1», как было с next);
//! 3) изменение, пришедшее ВО ВРЕМЯ прохода, не теряется: после прохода — ещё одно пробуждение с новым значением;
//! 4) рассылка закрыта → None (задача завершается).
//!
//! Кто вызывает: cargo test --test settings_coalesce_279.
//! От чего зависит: arb::settings (настоящее хранилище на SQLite во временной папке), arb::storage. Моков нет; данных
//! площадок нет — проверяется только механизм рассылки настроек.

use arb::settings::live::Watch;
use arb::settings::read::At;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use futures_util::FutureExt;
use serde_json::json;
use tokio::sync::broadcast;

/// Настройка из каталога (dex.*), которую меняем; область — площадка bsc_dex.
const NAME: &str = "dex.load_batch";
const VENUE: &str = "bsc_dex";

/// Новая база во временной папке. Возвращает хранилище и рассылку изменений (для подписок Watch).
async fn fresh(name: &str) -> (Store, broadcast::Sender<arb::settings::live::Change>) {
    let dir = std::env::temp_dir().join(format!("arb-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(1024);
    let (jtx, _) = broadcast::channel(16);
    let journal = arb::storage::journal::Journal::new(db.clone(), jtx);
    (Store::load(db, tx.clone(), journal).await.unwrap(), tx)
}

/// Записывает значение NAME. Ошибка записи — паника теста.
async fn put(s: &Store, v: u64) {
    let scope = Scope::Venue(VENUE.into());
    s.set(NAME, scope, Some(json!(v)), "тест").await.unwrap();
}

/// «Проход»: читает значение из хранилища в момент прохода (как pass() читает ctx.int). Ошибка — паника теста.
fn pass(s: &Store) -> u64 {
    let at = At {
        venue: Some(VENUE),
        coin: None,
    };
    s.integer(NAME, at).unwrap()
}

/// Есть ли пробуждение прямо сейчас (без ожидания): Some(склеено) / None — ждать нечего.
fn ready(w: &mut Watch) -> Option<usize> {
    w.next_batch().now_or_never().flatten()
}

#[tokio::test]
async fn batch_gives_one_pass_with_last_value() {
    let (s, tx) = fresh("coalesce279-batch").await;
    let mut w = Watch::new(tx.subscribe(), vec!["dex.".into()]);
    let n = 7;
    for v in 1..=n {
        put(&s, v).await;
        // Постороннее изменение (не dex.*) между ними — подписка его не видит.
        let other = Scope::Venue("bingx".into());
        s.set("feed.stale_after_ms", other, Some(json!(1000 + v)), "тест")
            .await
            .unwrap();
    }
    let mut seen = Vec::new();
    let mut wakes = Vec::new();
    while let Some(k) = ready(&mut w) {
        wakes.push(k);
        seen.push(pass(&s));
    }
    assert_eq!(
        wakes,
        vec![n as usize],
        "пачка из {n} изменений — одно пробуждение"
    );
    assert_eq!(seen, vec![n], "проход видит итоговое значение пачки");
}

#[tokio::test]
async fn single_change_one_pass_and_change_during_pass_not_lost() {
    let (s, tx) = fresh("coalesce279-single").await;
    let mut w = Watch::new(tx.subscribe(), vec!["dex.".into()]);
    // «1 в 1»: одно изменение — одно пробуждение, больше ничего.
    put(&s, 10).await;
    assert_eq!(ready(&mut w), Some(1));
    // Проход начался и прочитал 10; во время прохода пришло изменение на 20.
    assert_eq!(pass(&s), 10);
    put(&s, 20).await;
    // После прохода — ещё одно пробуждение, и проход видит 20.
    assert_eq!(
        ready(&mut w),
        Some(1),
        "изменение во время прохода даёт ещё проход"
    );
    assert_eq!(pass(&s), 20);
    assert_eq!(ready(&mut w), None, "больше проходов нет");
}

#[tokio::test]
async fn closed_feed_ends_the_loop() {
    let (tx, _) = broadcast::channel(4);
    let mut w = Watch::new(tx.subscribe(), vec!["dex.".into()]);
    drop(tx);
    assert_eq!(w.next_batch().await, None);
}
