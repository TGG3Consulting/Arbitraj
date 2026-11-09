//! Файл: tests/settings_venue_dir.rs
//! Что проверяет: решение 242 — область «площадка + тип направления» в хранилище настроек на настоящей SQLite:
//! 1) ключи: «venue_dir:bingx:cex_dex», «venue_dir:dex_dex» туда-обратно; неверные id не разбираются; старые ключи
//!    (global, venue, coin, event, dex, channel) читаются как раньше;
//! 2) проверка записи: тип направления должен быть у настройки (dir_kinds), у прочих настроек области нет;
//! 3) пусто = как раньше: для всех пяти настроек и любых пар decimal_dir/resolve_dir дают ровно то же, что decimal/resolve
//!    (значение и ошибка), в том числе когда заданы значения других пар;
//! 4) значение пары действует только на свою пару; очистка — снова прежняя цепочка; история одной области;
//! 5) перезапуск (новая загрузка той же базы) — старые и новые значения на месте (новый вид области — без переноса
//!    данных: столбец scope — текст, старые строки не трогаются);
//! 6) экспорт/импорт: те же шаги, что api/settings_io.rs (Scope::to_key, json_to_toml, TOML, Scope::from_key,
//!    toml_to_json, Store::set) переносят значения пары в новую базу один-в-один.
//! 7) дополнение 09.11: пара проверяется по видам площадок шлюза (неизвестная площадка, тип биржи у DEX — отказ);
//! 8) дополнение 09.11: у profit.price_move_k монета сильнее пары (монета → пара → вся система).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings, arb::api::settings_io_fmt, arb::storage. Моков нет — настоящая база.

use arb::api::settings_io_fmt as fmt;
use arb::settings::read::At;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::settings::venue_dir::{DirKind, VenueDir, id, parse_id};
use arb::storage::db::Db;
use serde_json::{Value, json};
use tokio::sync::broadcast;

const FIVE: [&str; 5] = [
    "money.min_profit_usd",
    "money.min_profit_pct",
    "money.max_per_trade_usd",
    "profit.price_move_k",
    "profit.closure_share",
];
const COIN: &str = "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82";

/// Папка базы теста. Принимает: имя.
fn dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("arb-vd-{name}-{}", std::process::id()))
}

/// Хранилище на базе в папке (create — начисто). Возвращает хранилище.
async fn open(name: &str, create: bool) -> Store {
    let p = dir(name);
    if create {
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
    }
    let db = Db::open(&p.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = arb::storage::journal::Journal::new(db.clone(), jtx);
    Store::load(db, tx, journal).await.unwrap()
}

async fn set(s: &Store, name: &str, scope: Scope, v: Value) {
    s.set(name, scope, Some(v), "тест").await.unwrap();
}

fn vd(venue: &str, kind: DirKind) -> Option<VenueDir<'_>> {
    Some(VenueDir { venue, kind })
}

/// Все пары, на которых проверяется «пусто = как раньше».
fn pairs() -> Vec<Option<VenueDir<'static>>> {
    let mut out = vec![None];
    for v in ["bingx", "mexc", "bsc_dex"] {
        for k in DirKind::ALL {
            out.push(vd(v, k));
        }
    }
    out
}

/// Все области чтения прежней цепочки.
fn ats() -> Vec<At<'static>> {
    let mut out = vec![At::default()];
    for v in [Some("bingx"), Some("mexc"), Some("bsc_dex"), None] {
        for c in [Some(COIN), None] {
            out.push(At { venue: v, coin: c });
        }
    }
    out
}

/// decimal_dir и resolve_dir для всех пар, кроме skip, совпадают с прежним чтением (значение или код и текст ошибки).
fn assert_same_as_before(s: &Store, skip: Option<VenueDir>) {
    for name in FIVE {
        for p in pairs() {
            if let (Some(sk), Some(pp)) = (skip, p)
                && sk.scope() == pp.scope()
            {
                continue;
            }
            for at in ats() {
                let old = s.decimal(name, at).map_err(|e| (e.code, e.message));
                let new = s.decimal_dir(name, p, at).map_err(|e| (e.code, e.message));
                assert_eq!(old, new, "{name} {p:?} {at:?}");
                assert_eq!(
                    s.resolve(name, at.venue, at.coin),
                    s.resolve_dir(name, p, at.venue, at.coin),
                    "{name} {p:?} {at:?}"
                );
            }
        }
    }
}

#[test]
fn keys_round_trip_and_old_keys_unchanged() {
    for (key, scope) in [
        (
            "venue_dir:bingx:cex_dex",
            Scope::VenueDir("bingx:cex_dex".into()),
        ),
        (
            "venue_dir:mexc:dex_cex",
            Scope::VenueDir("mexc:dex_cex".into()),
        ),
        (
            "venue_dir:bingx:cex_cex",
            Scope::VenueDir("bingx:cex_cex".into()),
        ),
        ("venue_dir:dex_dex", Scope::VenueDir("dex_dex".into())),
        ("global", Scope::Global),
        ("venue:bingx", Scope::Venue("bingx".into())),
        (&format!("coin:{COIN}"), Scope::Coin(COIN.into())),
        ("event:deal.closed", Scope::Event("deal.closed".into())),
        ("dex:pancakeswap_v3", Scope::Dex("pancakeswap_v3".into())),
        ("channel:club48", Scope::Channel("club48".into())),
    ] {
        assert_eq!(Scope::from_key(key), Some(scope.clone()), "{key}");
        assert_eq!(scope.to_key(), key);
    }
    for bad in [
        "venue_dir:bingx:dex_dex",
        "venue_dir::cex_dex",
        "venue_dir:bingx:xx",
        "venue_dir:bingx",
        "venue_dir:",
    ] {
        assert_eq!(Scope::from_key(bad), None, "{bad}");
    }
    // DEX ↔ DEX — один набор на все циклы: площадка в id не входит.
    assert_eq!(id("bsc_dex", DirKind::DexDex), "dex_dex");
    assert_eq!(id("other_dex", DirKind::DexDex), "dex_dex");
    assert_eq!(parse_id("dex_dex"), Some((None, DirKind::DexDex)));
    assert_eq!(
        parse_id("bingx:dex_cex"),
        Some((Some("bingx"), DirKind::DexToCex))
    );
}

#[tokio::test]
async fn validation_by_dir_kinds() {
    let s = open("valid", true).await;
    let ok = |n: &str, id: &str| {
        let s = s.clone();
        let (n, id) = (n.to_string(), id.to_string());
        async move {
            s.set(&n, Scope::VenueDir(id), Some(json!("1")), "тест")
                .await
        }
    };
    for n in FIVE {
        for k in ["bingx:cex_dex", "bingx:dex_cex", "mexc:cex_cex"] {
            assert!(ok(n, k).await.is_ok(), "{n} {k}");
        }
    }
    // DEX ↔ DEX — только лимит на сделку (у цикла порог — dex_dex.min_profit_usd, перевода и запаса нет).
    assert!(ok("money.max_per_trade_usd", "dex_dex").await.is_ok());
    for n in [
        "money.min_profit_usd",
        "money.min_profit_pct",
        "profit.price_move_k",
        "profit.closure_share",
    ] {
        let e = ok(n, "dex_dex").await.unwrap_err().to_string();
        assert!(e.contains("dex_dex"), "{e}");
    }
    // У других настроек «Деньги и риск» области нет; неверный id — отказ.
    assert!(ok("money.max_per_coin_usd", "bingx:cex_dex").await.is_err());
    assert!(
        ok("money.max_in_transit_usd", "bingx:cex_dex")
            .await
            .is_err()
    );
    assert!(ok("money.min_profit_usd", "bingx:dex_dex").await.is_err());
    assert!(ok("money.min_profit_usd", ":cex_dex").await.is_err());
}

#[tokio::test]
async fn empty_pairs_read_exactly_as_before_and_pair_applies_only_to_itself() {
    let s = open("same", true).await;
    // Ничего не задано: ошибки те же.
    assert_same_as_before(&s, None);
    // Прежние значения во всех прежних областях.
    set(&s, "money.min_profit_usd", Scope::Global, json!("0.3")).await;
    set(&s, "money.min_profit_pct", Scope::Global, json!("0.5")).await;
    set(&s, "money.max_per_trade_usd", Scope::Global, json!("100")).await;
    set(
        &s,
        "money.max_per_trade_usd",
        Scope::Venue("bsc_dex".into()),
        json!("40"),
    )
    .await;
    set(&s, "profit.price_move_k", Scope::Global, json!("1.5")).await;
    set(
        &s,
        "profit.price_move_k",
        Scope::Coin(COIN.into()),
        json!("2"),
    )
    .await;
    set(
        &s,
        "profit.closure_share",
        Scope::Venue("bingx".into()),
        json!("0.39"),
    )
    .await;
    assert_same_as_before(&s, None);
    // Значение одной пары: действует только на неё, остальные пары — как раньше.
    let pair = VenueDir {
        venue: "bingx",
        kind: DirKind::CexToDex,
    };
    for n in FIVE {
        set(&s, n, pair.scope(), json!("0.7")).await;
    }
    assert_same_as_before(&s, Some(pair));
    for n in FIVE {
        for at in ats() {
            assert_eq!(
                s.decimal_dir(n, Some(pair), at).unwrap(),
                rust_decimal::Decimal::new(7, 1)
            );
        }
    }
    // Очистка — снова прежняя цепочка; история — только этой области.
    for n in FIVE {
        s.set(n, pair.scope(), None, "тест").await.unwrap();
    }
    assert_same_as_before(&s, None);
    let h = s
        .history_in(
            Some("money.min_profit_usd".into()),
            Some(pair.scope().to_key()),
            10,
        )
        .await
        .unwrap();
    assert_eq!(h.len(), 2, "задано и очищено");
    assert!(h.iter().all(|r| r["scope"] == "venue_dir:bingx:cex_dex"));
    let all = s
        .history(Some("money.min_profit_usd".into()), 10)
        .await
        .unwrap();
    assert_eq!(all.len(), 3, "глобальное + две записи пары");
}

#[tokio::test]
async fn reload_and_export_import_round_trip() {
    let s = open("reload", true).await;
    set(&s, "money.min_profit_usd", Scope::Global, json!("0.3")).await;
    set(
        &s,
        "money.max_per_trade_usd",
        Scope::Venue("bingx".into()),
        json!("50"),
    )
    .await;
    set(
        &s,
        "money.min_profit_usd",
        Scope::VenueDir("bingx:dex_cex".into()),
        json!("0.8"),
    )
    .await;
    set(
        &s,
        "money.max_per_trade_usd",
        Scope::VenueDir("dex_dex".into()),
        json!("25"),
    )
    .await;
    // Перезапуск: та же база, новая загрузка.
    let r = open("reload", false).await;
    let mut a = s.all();
    let mut b = r.all();
    a.sort_by_key(|x| (x.0.clone(), x.1.to_key()));
    b.sort_by_key(|x| (x.0.clone(), x.1.to_key()));
    assert_eq!(a, b);
    assert_eq!(a.len(), 4);
    // Экспорт → TOML → импорт в новую базу (шаги api/settings_io.rs).
    let file = fmt::File {
        version: fmt::FORMAT_VERSION,
        setting: a
            .iter()
            .map(|(n, sc, v)| fmt::Entry {
                name: n.clone(),
                scope: sc.to_key(),
                value: fmt::json_to_toml(v).unwrap(),
            })
            .collect(),
        secrets: None,
    };
    let text = toml::to_string_pretty(&file).unwrap();
    assert!(text.contains("venue_dir:bingx:dex_cex") && text.contains("venue_dir:dex_dex"));
    let parsed: fmt::File = toml::from_str(&text).unwrap();
    let t = open("import", true).await;
    for e in parsed.setting {
        let scope = Scope::from_key(&e.scope).unwrap();
        let v = fmt::toml_to_json(&e.value).unwrap();
        t.set(&e.name, scope, Some(v), "import").await.unwrap();
    }
    let mut c = t.all();
    c.sort_by_key(|x| (x.0.clone(), x.1.to_key()));
    assert_eq!(a, c);
}

/// Решение 242, дополнение (Тигран 09.11: согласовано): пара «площадка · тип направления» проверяется при записи по
/// видам площадок шлюза (settings/venue_kinds.rs; ядро сообщает их при запуске). Виды не сообщены — как раньше; сообщены
/// — неизвестная площадка и тип биржи у DEX отклоняются с причиной (в том числе импорт — тот же Store::set), «dex_dex»
/// с площадкой отклоняет разбор id; очистка неверной пары, записанной до проверки, проходит; перезапуск — значение на
/// месте (загрузка не проверяет).
#[tokio::test]
async fn pair_checked_against_gateway_kinds() {
    use arb::gateway::contract::VenueKind;
    let s = open("kinds", true).await;
    let n = "money.min_profit_usd";
    // Виды не сообщены — проверки вида нет (запись прежняя).
    set(&s, n, Scope::VenueDir("bsc_dex:cex_dex".into()), json!("1")).await;
    s.venue_kinds().set([
        ("bingx".to_string(), VenueKind::Cex),
        ("mexc".to_string(), VenueKind::Cex),
        ("bsc_dex".to_string(), VenueKind::Dex),
    ]);
    for k in ["cex_dex", "dex_cex", "cex_cex"] {
        let e = s
            .set(
                n,
                Scope::VenueDir(format!("bsc_dex:{k}")),
                Some(json!("2")),
                "тест",
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("«bsc_dex» — DEX") && e.contains("dex_dex"),
            "{e}"
        );
    }
    let e = s
        .set(
            n,
            Scope::VenueDir("gate:cex_dex".into()),
            Some(json!("2")),
            "импорт",
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("площадки «gate» нет в шлюзе") && e.contains("bingx, bsc_dex, mexc"),
        "{e}"
    );
    assert!(
        s.set(
            n,
            Scope::VenueDir("bsc_dex:dex_dex".into()),
            Some(json!("2")),
            "тест"
        )
        .await
        .is_err()
    );
    // Верные пары — как раньше.
    for id in ["bingx:cex_dex", "mexc:dex_cex", "bingx:cex_cex"] {
        set(&s, n, Scope::VenueDir(id.into()), json!("0.4")).await;
    }
    // «DEX ↔ DEX» — только у money.max_per_trade_usd (решение 242).
    let m = "money.max_per_trade_usd";
    set(&s, m, Scope::VenueDir("dex_dex".into()), json!("25")).await;
    // Записанная до проверки неверная пара видна после перезапуска и очищается.
    let r = open("kinds", false).await;
    let bad = Scope::VenueDir("bsc_dex:cex_dex".into());
    assert_eq!(r.get_exact(n, &bad), Some(json!("1")));
    s.set(n, bad.clone(), None, "тест").await.unwrap();
    assert_eq!(s.get_exact(n, &bad), None);
}

/// Решение 242, дополнение п. 2 (Тигран 09.11: согласовано): profit.price_move_k — монета → площадка · направление →
/// вся система (Store::decimal_coin_dir). Монета и пара заданы — монета; только пара — пара; ничего — вся система;
/// у прочих настроек пары (decimal_dir) — по-прежнему пара сильнее монеты.
#[tokio::test]
async fn price_move_k_coin_first() {
    let s = open("kcoin", true).await;
    let k = "profit.price_move_k";
    let p = vd("bingx", DirKind::CexToDex);
    let at = At {
        venue: None,
        coin: Some(COIN),
    };
    set(&s, k, Scope::Global, json!("1.5")).await;
    // Ничего, кроме всей системы, — вся система.
    assert_eq!(s.decimal_coin_dir(k, p, at).unwrap().to_string(), "1.5");
    // Только пара — пара.
    set(&s, k, Scope::VenueDir("bingx:cex_dex".into()), json!("3")).await;
    assert_eq!(s.decimal_coin_dir(k, p, at).unwrap().to_string(), "3");
    // Монета и пара — монета.
    set(&s, k, Scope::Coin(COIN.into()), json!("2")).await;
    assert_eq!(s.decimal_coin_dir(k, p, at).unwrap().to_string(), "2");
    // Другая пара или без пары — монета.
    assert_eq!(
        s.decimal_coin_dir(k, vd("mexc", DirKind::CexToDex), at)
            .unwrap()
            .to_string(),
        "2"
    );
    assert_eq!(s.decimal_coin_dir(k, None, at).unwrap().to_string(), "2");
    // Без монеты в области — пара, иначе вся система.
    assert_eq!(
        s.decimal_coin_dir(k, p, At::default()).unwrap().to_string(),
        "3"
    );
    // Прежнее чтение decimal_dir (остальные четыре настройки) — пара сильнее монеты, без изменений.
    assert_eq!(s.decimal_dir(k, p, at).unwrap().to_string(), "3");
    // Не задано нигде — та же ошибка, что у decimal.
    let s2 = open("kcoin2", true).await;
    assert_eq!(
        s2.decimal_coin_dir(k, p, at).unwrap_err().message,
        s2.decimal(k, at).unwrap_err().message
    );
}
