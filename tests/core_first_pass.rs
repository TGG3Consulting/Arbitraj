//! Файл: tests/core_first_pass.rs
//! Что делает: сводка первой проверки налогов в ядре на настоящей SQLite (core/first_pass.rs): пока первый проход не
//! закончен, вход в чёрный список не уведомляется по монете (причины копятся); конец прохода — ровно одна сводка
//! core.check_done с причинами (их число — notify.summary_reasons); дальше — снова посмонетные уведомления.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::storage, arb::settings, arb::notify. Площадок в реестре нет —
//! проверяется решение ядра; числа синтетические.

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::first_pass;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use tokio::sync::broadcast;

/// Ядро на базе во временной папке.
async fn start(name: &str) -> (Core, Store, Journal) {
    let d = std::env::temp_dir().join(format!("arb-core-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let (settings2, journal2) = (settings.clone(), journal.clone());
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db: db.clone(),
        changes,
        buffer: 64,
    })
    .await;
    (core, settings2, journal2)
}

#[tokio::test]
async fn first_pass_sends_one_summary_then_per_coin() {
    let (core, settings, journal) = start("first-pass").await;
    settings
        .set(
            "notify.summary_reasons",
            Scope::Global,
            Some(json!(1)),
            "тест",
        )
        .await
        .unwrap();
    // Первый проход: три монеты в списке — по монете не уведомляем.
    assert!(first_pass::absorb(&core, Some("откат пробы".into())));
    assert!(first_pass::absorb(&core, Some("откат пробы".into())));
    assert!(first_pass::absorb(&core, Some("налог выше порога".into())));
    let n = first_pass::Counts {
        ok: 10,
        failed: 3,
        waiting: 2,
        config: 0,
        loading: 4,
    };
    first_pass::done(&core, "bsc_dex", n).await;
    first_pass::done(&core, "bsc_dex", n).await;
    let list = journal.list(500, None).await.unwrap();
    let done: Vec<_> = list
        .iter()
        .filter(|e| e.kind == "core.check_done")
        .collect();
    assert_eq!(done.len(), 1, "сводка одна");
    // Монеты, чьи пулы застряли (ждут настройку), — отдельным числом (решение 63).
    assert!(
        done[0]
            .message
            .contains("пулы не читаются (ждут настройку) 4"),
        "{}",
        done[0].message
    );
    assert!(
        done[0].message.contains("в чёрном списке 3"),
        "{}",
        done[0].message
    );
    assert!(
        done[0].message.contains("2 — откат пробы"),
        "{}",
        done[0].message
    );
    assert!(
        !done[0].message.contains("налог выше порога"),
        "показана 1 причина (notify.summary_reasons)"
    );
    // После прохода — посмонетные уведомления.
    assert!(!first_pass::absorb(&core, Some("откат пробы".into())));
}
