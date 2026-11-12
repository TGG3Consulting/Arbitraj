//! Файл: tests/core_daily_pair.rs
//! Что проверяет: решение 258 (Тигран 12.11.2026: «Binance DEX и DEX Binance — это тоже разные направления. Их нужно
//! разбить. У каждого свой лимит. И так для всех бирж.»; «Лимит распространяется только на биржу в момент покупки. В
//! обратную сторону он не считается.»; «Каждое направление имеет свой лимит. Все.») на настоящем ядре и SQLite во
//! временной папке (реестр шлюза пуст, виды площадок — в inner.kinds; моков нет) и записанных сделках отчёта 05.11
//! (tests/data/report_2026-11-05/deals.csv: DM WKC −7.9757, MD ALEO −0.4853, KM ION −1.3198, BD CAKE −0.26,
//! DB TRADOOR −0.1878):
//! 1) каждая пара «площадка × тип направления» копит своё; сумма пар = общий итог;
//! 2) исчерпан «DEX → MEXC» (значение пары сильнее «вся система») — встаёт только DEX → MEXC; MEXC → DEX, BingX → MEXC
//!    (это «BingX → биржа»), MEXC → BingX, DEX → BingX торгуют;
//! 3) исчерпан «KuCoin → биржа» — не начинаются сделки с покупкой на KuCoin и продажей на другой бирже; MEXC → KuCoin,
//!    KuCoin → DEX, DEX → KuCoin торгуют;
//! 4) прочие паузы площадки (ошибки подряд) блокируют все её направления с обеих сторон; их снятие (как автоснятие —
//!    auto_pause::lift_venue) паузу направления по убытку не снимает;
//! 5) «Сбросить лимит дня» и «Снять паузу» по паре — только её; команда из Telegram по id пары;
//! 6) полночь (day_roll::new_day) снимает паузы пар;
//! 7) обязательность risk.max_daily_loss_usd — у пары сделки: пара → площадка пары → вся система.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, tests/report_common.

mod report_common;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::risk::{Mode, can_open};
use arb::core::types::{Asset, Direction, ManualAction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use report_common::{d, deal, dir, start};
use rust_decimal::Decimal;
use serde_json::json;

/// Площадка по id.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Задаёт настройку. Ошибок нет.
async fn set(st: &Store, name: &str, scope: Scope, val: &str) {
    st.set(name, scope, Some(json!(val)), "тест").await.unwrap();
}

/// Ядро: виды площадок, обязательные money.* и risk.* (risk.max_daily_loss_usd «вся система» — `limit`, None — не
/// задана), risk.max_errors_in_row = 3, «В работе».
async fn core_at(name: &str, limit: Option<&str>) -> (Core, Store) {
    let (core, st) = start(&dir(name)).await;
    *core.inner.kinds.write().unwrap() = vec![
        (v("bingx"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
        (v("kucoin"), VenueKind::Cex),
        (v("mexc"), VenueKind::Cex),
    ];
    // Те же виды — хранилищу настроек (проверка области VenueDir при записи, settings/venue_kinds.rs), как Core::start.
    st.venue_kinds().set([
        ("bingx".to_string(), VenueKind::Cex),
        ("bsc_dex".to_string(), VenueKind::Dex),
        ("kucoin".to_string(), VenueKind::Cex),
        ("mexc".to_string(), VenueKind::Cex),
    ]);
    for (n, x) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.min_bnb_balance", "0.01"),
    ] {
        set(&st, n, Scope::Global, x).await;
    }
    if let Some(l) = limit {
        set(&st, "risk.max_daily_loss_usd", Scope::Global, l).await;
    }
    for id in ["bingx", "bsc_dex", "kucoin", "mexc"] {
        st.set(
            "risk.max_errors_in_row",
            Scope::Venue(id.into()),
            Some(json!(3)),
            "тест",
        )
        .await
        .unwrap();
    }
    core.set_mode(Mode::Running).await;
    (core, st)
}

/// Направление на монете.
fn dir_of(buy: &str, sell: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: v(buy),
        sell_on: v(sell),
    }
}

/// Записанная сделка `k` закрыта ядром (ledger::finish).
async fn close(core: &Core, k: usize) -> Deal {
    let mut x = deal(k, "p");
    arb::core::ledger::finish(core, &mut x, Step::Closed, "запись 05.11").await;
    x
}

/// Итог дня пары (нет строки — 0).
fn res(core: &Core, p: &str) -> Decimal {
    core.control()
        .day
        .pairs
        .get(p)
        .map_or(Decimal::ZERO, |x| x.result_usd)
}

/// Открытие направления: Ok или текст отказа.
fn open(core: &Core, buy: &str, sell: &str) -> Result<(), String> {
    can_open(core, &dir_of(buy, sell))
}

#[tokio::test]
async fn each_pair_accumulates_its_own() {
    let (core, _) = core_at("pair-own", Some("100")).await;
    for k in [0, 108, 359, 379, 150] {
        close(&core, k).await;
    }
    assert_eq!(res(&core, "bingx:cex_dex"), d("-0.26"));
    assert_eq!(res(&core, "mexc:dex_cex"), d("-7.9757"));
    assert_eq!(res(&core, "mexc:cex_dex"), d("-0.4853"));
    assert_eq!(res(&core, "kucoin:cex_cex"), d("-1.3198"));
    assert_eq!(res(&core, "bingx:dex_cex"), d("-0.1878"));
    for p in [
        "mexc:cex_cex",
        "kucoin:cex_dex",
        "kucoin:dex_cex",
        "bingx:cex_cex",
        "dex_dex",
    ] {
        assert_eq!(res(&core, p), Decimal::ZERO, "{p}");
    }
    let r = core.control();
    let sum: Decimal = r.day.pairs.values().map(|x| x.result_usd).sum();
    assert_eq!(sum, r.day.result_usd, "сумма пар = общий итог");
    // Пары реестра есть и без сделок: у биржи три, у DEX одна общая.
    assert_eq!(r.day.pairs.len(), 10);
    assert_eq!(r.day.pairs["mexc:dex_cex"].label, "DEX → MEXC");
    assert_eq!(r.day.pairs["mexc:dex_cex"].venue.as_deref(), Some("mexc"));
    assert_eq!(r.day.pairs["dex_dex"].venue, None);
    assert!(r.pair_paused.is_empty());
}

#[tokio::test]
async fn dex_to_mexc_exhausted_pauses_only_it() {
    let (core, st) = core_at("pair-dexmexc", Some("100")).await;
    // Значение пары «DEX → MEXC» сильнее «вся система».
    set(
        &st,
        "risk.max_daily_loss_usd",
        Scope::VenueDir("mexc:dex_cex".into()),
        "5",
    )
    .await;
    close(&core, 108).await; // DEX → MEXC, WKC, −7.9757
    let r = core.control();
    assert_eq!(
        r.pair_paused.keys().collect::<Vec<_>>(),
        vec!["mexc:dex_cex"]
    );
    assert!(
        r.pair_paused["mexc:dex_cex"]
            .starts_with("DEX → MEXC: убыток за день 7.98 USDT больше лимита 5.00 USDT"),
        "{}",
        r.pair_paused["mexc:dex_cex"]
    );
    assert!(r.venue_paused.is_empty());
    assert_eq!(
        r.day.pairs["mexc:dex_cex"].limit.as_ref().unwrap().max_usd,
        d("5")
    );
    assert_eq!(
        r.day.pairs["mexc:cex_dex"].limit.as_ref().unwrap().max_usd,
        d("100")
    );
    let why = open(&core, "bsc_dex", "mexc").unwrap_err();
    assert!(why.contains("направление DEX → MEXC на паузе"), "{why}");
    for (b, s) in [
        ("mexc", "bsc_dex"),
        ("bingx", "mexc"),
        ("mexc", "bingx"),
        ("bsc_dex", "bingx"),
    ] {
        assert_eq!(open(&core, b, s), Ok(()), "{b} → {s}");
    }
    // Минус другого направления той же биржи копится у него и паузу не ставит.
    close(&core, 359).await; // MEXC → DEX, −0.4853
    assert_eq!(res(&core, "mexc:cex_dex"), d("-0.4853"));
    assert_eq!(core.control().pair_paused.len(), 1);
}

#[tokio::test]
async fn kucoin_cex_cex_exhausted_blocks_only_buy_on_kucoin() {
    let (core, st) = core_at("pair-kucoin", Some("100")).await;
    // Значение площадки действует для всех её направлений (пара → площадка → вся система).
    set(
        &st,
        "risk.max_daily_loss_usd",
        Scope::Venue("kucoin".into()),
        "1",
    )
    .await;
    close(&core, 379).await; // KuCoin → MEXC, ION, −1.3198
    let r = core.control();
    assert_eq!(
        r.pair_paused.keys().collect::<Vec<_>>(),
        vec!["kucoin:cex_cex"]
    );
    for s in ["mexc", "bingx"] {
        let why = open(&core, "kucoin", s).unwrap_err();
        assert!(why.contains("направление KuCoin → биржа на паузе"), "{why}");
    }
    for (b, s) in [
        ("mexc", "kucoin"),
        ("bingx", "kucoin"),
        ("kucoin", "bsc_dex"),
        ("bsc_dex", "kucoin"),
    ] {
        assert_eq!(open(&core, b, s), Ok(()), "{b} → {s}");
    }
}

#[tokio::test]
async fn other_venue_pauses_block_all_its_directions() {
    let (core, st) = core_at("pair-other", Some("100")).await;
    set(
        &st,
        "risk.max_daily_loss_usd",
        Scope::VenueDir("mexc:dex_cex".into()),
        "5",
    )
    .await;
    close(&core, 108).await; // пауза DEX → MEXC по убытку
    // Ошибки подряд у MEXC — пауза всей площадки: все её направления с обеих сторон.
    for _ in 0..3 {
        arb::core::risk::on_error(&core, &v("mexc"));
    }
    for (b, s) in [
        ("mexc", "bsc_dex"),
        ("bsc_dex", "mexc"),
        ("bingx", "mexc"),
        ("mexc", "bingx"),
    ] {
        let why = open(&core, b, s).unwrap_err();
        assert!(why.contains("площадка mexc на паузе"), "{b} → {s}: {why}");
    }
    assert_eq!(open(&core, "bingx", "bsc_dex"), Ok(()));
    // Снятие паузы площадки, как автоснятие (auto_pause::lift_venue): пауза направления по убытку остаётся.
    {
        let mut r = core.inner.risk.lock().unwrap();
        let m = r.venue_auto[&v("mexc")].clone();
        assert!(arb::core::auto_pause::lift_venue(&mut r, &v("mexc"), &m));
    }
    assert!(
        open(&core, "bsc_dex", "mexc")
            .unwrap_err()
            .contains("DEX → MEXC")
    );
    assert_eq!(open(&core, "mexc", "bsc_dex"), Ok(()));
    assert_eq!(open(&core, "bingx", "mexc"), Ok(()));
    // Полночь (UTC): паузы пар сняты.
    let lift = arb::core::day_roll::new_day(&mut core.inner.risk.lock().unwrap());
    assert_eq!(lift.pairs, vec!["mexc:dex_cex".to_string()]);
    assert_eq!(open(&core, "bsc_dex", "mexc"), Ok(()));
}

#[tokio::test]
async fn reset_and_resume_by_pair_only_it() {
    let (core, _) = core_at("pair-reset", Some("0.4")).await;
    close(&core, 108).await; // DEX → MEXC −7.9757
    close(&core, 359).await; // MEXC → DEX −0.4853
    let r = core.control();
    assert!(
        r.pair_paused.contains_key("mexc:dex_cex") && r.pair_paused.contains_key("mexc:cex_dex")
    );
    let t = core.reset_venue_daily_limit("mexc:dex_cex", "тест").await;
    assert!(
        t.contains("Лимит дня направления DEX → MEXC сброшен"),
        "{t}"
    );
    let r = core.control();
    assert!(!r.pair_paused.contains_key("mexc:dex_cex"));
    assert!(r.pair_paused.contains_key("mexc:cex_dex"));
    let x = &r.day.pairs["mexc:dex_cex"];
    assert_eq!(x.limit.as_ref().unwrap().loss_usd, Decimal::ZERO);
    assert_eq!(x.result_usd, d("-7.9757"));
    assert!(r.day.pairs["mexc:cex_dex"].limit_reset.is_none());
    // «Снять паузу» по паре из Telegram — тот же путь команд ядра.
    let a = ManualAction::ResumeVenue {
        venue: "mexc:cex_dex".into(),
    };
    let t = arb::core::commands_control::run(&core, &a, "тест")
        .await
        .unwrap();
    assert_eq!(t, "пауза направления MEXC → DEX снята");
    assert!(core.control().pair_paused.is_empty());
    let t = arb::core::commands_control::run(&core, &a, "тест")
        .await
        .unwrap();
    assert!(t.contains("не на паузе"), "{t}");
    // Новый минус после «Снять паузу» без сброса — снова пауза только этой пары.
    close(&core, 315).await; // MEXC → DEX −0.4179
    assert_eq!(
        core.control().pair_paused.keys().collect::<Vec<_>>(),
        vec!["mexc:cex_dex"]
    );
}

#[tokio::test]
async fn setting_required_for_pair_of_deal() {
    let (core, st) = core_at("pair-required", None).await;
    let why = open(&core, "bsc_dex", "mexc").unwrap_err();
    assert!(
        why.contains("risk.max_daily_loss_usd [venue_dir:mexc:dex_cex]"),
        "{why}"
    );
    // Значение только пары — достаточно для неё, не для соседней.
    set(
        &st,
        "risk.max_daily_loss_usd",
        Scope::VenueDir("mexc:dex_cex".into()),
        "5",
    )
    .await;
    assert_eq!(open(&core, "bsc_dex", "mexc"), Ok(()));
    let why = open(&core, "mexc", "bsc_dex").unwrap_err();
    assert!(why.contains("[venue_dir:mexc:cex_dex]"), "{why}");
    // Значение площадки MEXC — для всех её направлений (в том числе DEX → MEXC), не для BingX.
    set(
        &st,
        "risk.max_daily_loss_usd",
        Scope::Venue("mexc".into()),
        "3",
    )
    .await;
    assert_eq!(open(&core, "mexc", "bsc_dex"), Ok(()));
    assert_eq!(open(&core, "mexc", "kucoin"), Ok(()));
    assert!(
        open(&core, "bingx", "mexc")
            .unwrap_err()
            .contains("[venue_dir:bingx:cex_cex]")
    );
    // DEX ↔ DEX — своя пара; значение площадки DEX подходит.
    assert!(
        open(&core, "bsc_dex", "bsc_dex")
            .unwrap_err()
            .contains("[venue_dir:dex_dex]")
    );
    set(
        &st,
        "risk.max_daily_loss_usd",
        Scope::Venue("bsc_dex".into()),
        "2",
    )
    .await;
    assert_eq!(open(&core, "bsc_dex", "bsc_dex"), Ok(()));
    // Значение «вся система» — для всех.
    set(&st, "risk.max_daily_loss_usd", Scope::Global, "10").await;
    assert_eq!(open(&core, "bingx", "mexc"), Ok(()));
}
