//! Файл: tests/core_deposit_warm_316.rs
//! Что делает: решение 316 — чистая логика прогрева адресов депозита (core/deposit_warm_plan.rs), без запросов к биржам:
//! - pick: только направления, где биржа — площадка продажи B, по одному на монету, в порядке справочника; биржа
//!   занята сделкой (Busy) — ни одного (прогрев уступает сделке, запросов нет), другие биржи — как обычно;
//! - after: отказ доступа, аккаунта, лимит, связь, адаптер не подключён — стоп прохода по бирже; отказ по монете
//!   (InvalidAddress, NotApplicable — создание запрещено, BadResponse) — дальше;
//! - Memo: полученный адрес больше не запрашивается; отказ по монете — до forget_failed; стоп и BadResponse — не
//!   запоминаются; NotApplicable — отказ, не «готов»;
//! - reason: короткая причина по коду; Tally: строка журнала без адресов; проход, упёршийся в закрытые секреты, журнал
//!   не пишет; проход, уступивший сделке, — с причиной остановки.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::deposit_warm_plan, arb::core::deposit_warm_busy (Busy), arb::core::types,
//! arb::gateway::errors. Моков нет: ответов бирж тест
//! не использует — только типы ядра (монеты называются по живому факту 16.11, сделка SHOPINX bsc_dex → bingx).

use arb::core::deposit_warm_busy::Busy;
use arb::core::deposit_warm_plan::{After, Memo, Tally, after, pick, reason};
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::errors::ErrorCode;

/// Направление монеты `t` с покупкой на `a` и продажей на `b`.
fn dir(t: &str, a: &str, b: &str) -> Direction {
    Direction {
        asset: Asset {
            chain: None,
            ticker: t.into(),
            contract: None,
        },
        buy_on: VenueId(a.into()),
        sell_on: VenueId(b.into()),
    }
}

/// Ключ (тикер на бирже, сеть) — текст сети здесь только ключ памяти, к бирже не уходит.
fn key(t: &str) -> (String, String) {
    (t.into(), "BSC".into())
}

#[test]
fn pick_only_sell_side_one_per_coin() {
    let bingx = VenueId("bingx".into());
    let dirs = vec![
        dir("SHOPINX", "bsc_dex", "bingx"),
        dir("SHOPINX", "bingx", "bsc_dex"),
        dir("SHOPINX", "mexc", "bingx"),
        dir("CAKE", "bsc_dex", "mexc"),
        dir("CAKE", "bsc_dex", "bingx"),
    ];
    let got: Vec<String> = pick(dirs, &bingx, &Busy::default())
        .into_iter()
        .map(|d| d.asset.ticker)
        .collect();
    assert_eq!(got, vec!["SHOPINX".to_string(), "CAKE".to_string()]);
}

#[test]
fn stop_on_access_rate_and_link() {
    for c in [
        ErrorCode::SecretsLocked,
        ErrorCode::NotConfigured,
        ErrorCode::InvalidCredentials,
        ErrorCode::InsufficientPermissions,
        ErrorCode::IpNotAllowed,
        ErrorCode::AccountRestricted,
        ErrorCode::RateLimited,
        ErrorCode::Timeout,
        ErrorCode::Unreachable,
        ErrorCode::VenueNotConnected,
    ] {
        assert_eq!(after(&c), After::Stop, "{c:?}");
    }
    for c in [
        ErrorCode::InvalidAddress,
        ErrorCode::NotApplicable,
        ErrorCode::BadResponse,
    ] {
        assert_eq!(after(&c), After::Next, "{c:?}");
    }
}

#[test]
fn busy_venue_gets_no_requests() {
    let bingx = VenueId("bingx".into());
    let mexc = VenueId("mexc".into());
    let dirs = vec![
        dir("SHOPINX", "bsc_dex", "bingx"),
        dir("CAKE", "bsc_dex", "mexc"),
    ];
    // Сделка SHOPINX bsc_dex → bingx идёт: bingx занята, прогрев ей запросов не шлёт.
    let mut busy = Busy::default();
    busy.add_direction(&dir("SHOPINX", "bsc_dex", "bingx"));
    assert!(busy.has(&bingx));
    assert!(pick(dirs.clone(), &bingx, &busy).is_empty());
    // Биржа, не занятая сделкой, прогревается как обычно.
    let got: Vec<String> = pick(dirs, &mexc, &busy)
        .into_iter()
        .map(|d| d.asset.ticker)
        .collect();
    assert_eq!(got, vec!["CAKE".to_string()]);
}

#[test]
fn bad_response_retried_not_applicable_refused() {
    let mut m = Memo::default();
    // Ответ не разобран — дальше по монетам, но монета не «отказная»: повтор на следующем толчке.
    assert_eq!(
        m.record(key("SHOPINX"), Err(&ErrorCode::BadResponse)),
        After::Next
    );
    assert!(m.todo(&key("SHOPINX")));
    // Создание адреса запрещено прогревом — отказ по монете, не «готов».
    assert_eq!(
        m.record(key("CAKE"), Err(&ErrorCode::NotApplicable)),
        After::Next
    );
    assert!(!m.todo(&key("CAKE")));
    assert_eq!(m.counts(), (0, 1));
    // Аккаунт ограничен — стоп прохода, ключ не запоминается.
    assert_eq!(
        m.record(key("TAG"), Err(&ErrorCode::AccountRestricted)),
        After::Stop
    );
    assert!(m.todo(&key("TAG")));
}

#[test]
fn reason_has_no_values() {
    for c in [
        ErrorCode::InvalidAddress,
        ErrorCode::NotApplicable,
        ErrorCode::BadResponse,
        ErrorCode::AccountRestricted,
    ] {
        let r = reason(&c);
        assert!(!r.is_empty(), "{c:?}");
        assert!(!r.contains('('), "{c:?}: {r}");
    }
}

#[test]
fn memo_ok_failed_and_stop() {
    let mut m = Memo::default();
    assert!(m.todo(&key("SHOPINX")));
    assert_eq!(m.record(key("SHOPINX"), Ok(())), After::Next);
    assert!(!m.todo(&key("SHOPINX")));
    // Нет адреса / memo — монета не запрашивается до forget_failed.
    assert_eq!(
        m.record(key("CAKE"), Err(&ErrorCode::InvalidAddress)),
        After::Next
    );
    assert!(!m.todo(&key("CAKE")));
    // Лимит — стоп, ключ не запоминается: повтор со следующим толчком.
    assert_eq!(
        m.record(key("TAG"), Err(&ErrorCode::RateLimited)),
        After::Stop
    );
    assert!(m.todo(&key("TAG")));
    assert_eq!(m.counts(), (1, 1));
    m.forget_failed();
    assert!(m.todo(&key("CAKE")));
    assert!(!m.todo(&key("SHOPINX")));
    assert_eq!(m.counts(), (1, 0));
}

#[test]
fn tally_text_and_silence() {
    assert!(Tally::default().silent());
    let locked = Tally {
        asked: 1,
        failed: 1,
        locked: true,
        stopped: Some("SecretsLocked".into()),
        ..Tally::default()
    };
    assert!(locked.silent());
    let t = Tally {
        asked: 3,
        got: 2,
        failed: 1,
        first_err: Some("CAKE: нет адреса".into()),
        ..Tally::default()
    };
    assert!(!t.silent());
    let s = t.text("bingx", 2, 1);
    assert!(s.contains("запрошено 3, получено 2, отказов 1"), "{s}");
    assert!(s.contains("первая причина — CAKE: нет адреса"), "{s}");
    assert!(!s.contains("прерван"), "{s}");
}

#[test]
fn tally_stopped_text() {
    let t = Tally {
        asked: 2,
        got: 1,
        failed: 1,
        first_err: Some("CAKE: InvalidAddress — адреса нет".into()),
        stopped: Some("биржа занята сделкой — продолжение, когда освободится".into()),
        ..Tally::default()
    };
    assert!(!t.silent());
    let s = t.text("mexc", 1, 1);
    assert!(s.contains("проход прерван (биржа занята сделкой"), "{s}");
    // Проход, прерванный до первого запроса, журнал не пишет.
    let quiet = Tally {
        stopped: Some("RateLimited".into()),
        ..Tally::default()
    };
    assert!(quiet.silent());
}
