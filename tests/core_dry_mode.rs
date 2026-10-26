//! Файл: tests/core_dry_mode.rs
//! Что делает: сухой режим (решение 74) на настоящем ядре и SQLite во временной папке: dry.enabled = да — вход
//! исполнителя (executor::accepted) отказывает первым же шагом, ничего не запускает и не пишет сделок (расчёт —
//! записанные CAKE: asks BingX, пул V2); тот же расчёт без сухого режима проходит этот замок и останавливается дальше
//! («направление не торгуемо» — площадок в реестре нет), то есть отказ в сухом режиме дал именно он; режим читается на
//! лету; dry.enabled не задана — обычный режим; включён без dry.budget_usdt — бюджета нет, причина названа; ссылки
//! карточки по шаблонам из настроек (пара — по площадке, пул — по DEX).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, tests/size_common. Моков нет: площадок в
//! реестре нет, проверяется решение ядра.

mod size_common;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::dry::{self, Mode};
use arb::core::executor::{self, Start};
use arb::core::profit::{self, Verdict};
use arb::core::types::VenueId;
use arb::core::{candidates, limits, precheck};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use size_common::{Rec, d};
use tokio::sync::broadcast;

/// Ядро на базе во временной папке.
async fn start(name: &str) -> (Core, Store) {
    start_with(name, None).await
}

/// Ядро; raw — значение, записанное в базу мимо проверки (повреждение): (имя, JSON-текст).
async fn start_with(name: &str, raw: Option<(&str, &str)>) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-dry-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    if let Some((n, v)) = raw {
        let (n, v) = (n.to_string(), v.to_string());
        db.call(move |c| {
            c.execute(
                "INSERT INTO settings (name, scope, value, updated_at) VALUES (?1, 'global', ?2, '2026-10-26T00:00:00Z')",
                rusqlite::params![n, v],
            )
        })
        .await
        .unwrap();
    }
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
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    (core, settings)
}

/// Задаёт или очищает настройку всей системы.
async fn set(s: &Store, name: &str, v: Option<serde_json::Value>) {
    s.set(name, Scope::Global, v, "тест").await.unwrap();
}

#[tokio::test]
async fn dry_mode_never_calls_executor() {
    let (core, s) = start("gate").await;
    let rec = Rec::load("dry-gate").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    // Не задано — обычный режим.
    assert_eq!(dry::mode(&s), Mode::Live);
    set(&s, "dry.enabled", Some(json!(true))).await;
    set(&s, "dry.budget_usdt", Some(json!("1000"))).await;
    assert_eq!(dry::mode(&s), Mode::Dry(Ok(d("1000"))));
    assert!(dry::blocks(&s).is_some());
    let r = executor::accepted(&core, calc.clone()).await;
    assert!(matches!(r, Start::Dry(_)), "{r:?}");
    assert!(core.deals().is_empty(), "сделки нет");
    // Включение снято на лету — замок пропускает, дальше отказ ядра (площадок в реестре нет).
    set(&s, "dry.enabled", Some(json!(false))).await;
    assert_eq!(dry::mode(&s), Mode::Live);
    assert_eq!(executor::accepted(&core, calc).await, Start::NotTradable);
    assert!(core.deals().is_empty());
}

#[tokio::test]
async fn dry_without_budget_has_reason() {
    let (_core, s) = start("budget").await;
    set(&s, "dry.enabled", Some(json!(true))).await;
    match dry::mode(&s) {
        Mode::Dry(Err(e)) => assert!(e.contains("dry.budget_usdt"), "{e}"),
        m => panic!("ожидался сухой режим без бюджета: {m:?}"),
    }
    // Без бюджета ничего не исполняется.
    assert!(dry::blocks(&s).is_some());
}

/// Ссылки карточки по настройкам (решение 74): не задан шаблон — причина; пара — по площадке, пул — по DEX.
#[tokio::test]
async fn link_templates_from_settings() {
    use arb::core::card_links::{pair, pool};
    use arb::core::types::PoolRef;
    let (_core, s) = start("links").await;
    let p = PoolRef::Contract(size_common::PAIR);
    let l = pair(&s, "bingx", "CAKE");
    assert!(l.url.is_none() && l.why.unwrap().contains("venue.pair_url_template"));
    let bx = Scope::Venue("bingx".into());
    s.set("venue.quote_asset", bx.clone(), Some(json!("USDT")), "тест")
        .await
        .unwrap();
    s.set(
        "venue.pair_url_template",
        bx,
        Some(json!("https://bingx.com/en/spot/{symbol}")),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(
        pair(&s, "bingx", "CAKE").url.as_deref(),
        Some("https://bingx.com/en/spot/CAKEUSDT")
    );
    let dex = "pancakeswap-v2-bsc";
    assert!(pool(&s, "bsc_dex", dex, &p).why.unwrap().contains(dex));
    s.set(
        "dex.pool_url_template",
        Scope::Dex(dex.into()),
        Some(json!("https://www.geckoterminal.com/bsc/pools/{pool}")),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(
        pool(&s, "bsc_dex", dex, &p).url.as_deref(),
        Some("https://www.geckoterminal.com/bsc/pools/0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b")
    );
    // Другая DEX — свой шаблон, этот не подставляется.
    assert!(pool(&s, "bsc_dex", "uniswap-v4-bsc", &p).url.is_none());
}

/// Гонка (ревью #64): режим зафиксирован при входе в сделку (обычный), сухой включили во время предпроверки — бюджет
/// живой сделки виртуальным не становится (limits::available в зафиксированном режиме настройку не читает), а замок
/// перед покупкой (executor::buy_gate) покупку не пускает.
#[tokio::test]
async fn live_deal_never_gets_virtual_budget() {
    let (core, s) = start("race").await;
    let bingx = VenueId("bingx".into());
    let fixed = dry::mode(&s);
    assert_eq!(fixed, Mode::Live);
    assert!(executor::buy_gate(&core).is_ok());
    set(&s, "dry.enabled", Some(json!(true))).await;
    set(&s, "dry.budget_usdt", Some(json!("1000"))).await;
    // Живая сделка: баланс — только через шлюз (площадки в реестре нет — ошибка, а не 1000 виртуальных).
    let live = limits::available(&core, &bingx, "USDT", &fixed).await;
    assert!(live.is_err(), "{live:?}");
    // Сухой расчёт кандидата — виртуальный бюджет, к площадке не ходит.
    let (v, from) = limits::available(&core, &bingx, "USDT", &dry::mode(&s))
        .await
        .unwrap();
    assert_eq!(v, d("1000"));
    assert!(from.contains("dry.budget_usdt"));
    assert!(
        executor::buy_gate(&core)
            .unwrap_err()
            .contains("остановлена: включён сухой режим до покупки")
    );
}

/// Повреждённое значение dry.enabled — режим сухой, причина видна (плашка и замки).
#[tokio::test]
async fn unreadable_flag_is_dry() {
    let (_core, s) = start_with("broken", Some(("dry.enabled", "\"да\""))).await;
    assert!(dry::mode(&s).is_dry());
    assert!(dry::blocks(&s).is_some());
    let st = dry::status(&s);
    assert_eq!(st.state, "broken");
    assert!(st.reason.unwrap().contains("dry.enabled"));
}

/// Путь очереди кандидатов в сухом режиме: прошедший помечен «сухой», исполнитель не вызывается, сделок нет.
#[tokio::test]
async fn candidates_accepted_in_dry_mode() {
    let (core, s) = start("cand").await;
    set(&s, "dry.enabled", Some(json!(true))).await;
    set(&s, "dry.budget_usdt", Some(json!("1000"))).await;
    let rec = Rec::load("dry-cand").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    let dir = calc.direction.clone();
    candidates::accepted(&core, &dir, calc, &dry::mode(&s)).await;
    assert!(core.deals().is_empty(), "сделки нет");
    let rows = core.candidates();
    let row = &rows.as_array().unwrap()[0];
    assert_eq!(row["dry"], true);
    assert!(row["verdict"].get("Accept").is_some());
}

/// Пересчёт живой сделки перед покупкой (precheck::replan) — всегда режим Live: при сухом режиме без бюджета расчёт в
/// сухом режиме отказывает причиной dry.budget_usdt, а пересчёт сделки — нет (идёт дальше и упирается в справочник).
/// Если пересчёт сделки начнёт брать режим из настроек, причина станет «dry.budget_usdt» и тест упадёт.
#[tokio::test]
async fn precheck_replans_live() {
    let (core, s) = start("replan").await;
    set(&s, "dry.enabled", Some(json!(true))).await;
    let dir = size_common::calc::direction();
    let reason = |v: Verdict| match v {
        Verdict::Reject { reason, .. } => reason,
        Verdict::Accept(_) => panic!("справочник пуст — прохода быть не может"),
    };
    let dry_reason = reason(profit::decide(&core, &dir, &dry::mode(&s)).await);
    assert!(dry_reason.contains("dry.budget_usdt"), "{dry_reason}");
    let live_reason = reason(precheck::replan(&core, &dir).await);
    assert!(
        !live_reason.contains("dry"),
        "пересчёт сделки — режим Live: {live_reason}"
    );
}
