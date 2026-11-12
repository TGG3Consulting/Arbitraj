//! Файл: tests/withdraw_limit_t85.rs
//! Что проверяет: решение 253, дополнение t85 (Тигран 11.11):
//! (1) лимит вывода аккаунта, который сбрасывается сам (суточный / месячный), — новый код шлюза WithdrawLimit у вывода
//!     по официальным таблицам ошибок бирж (код и текст — из таблицы, ссылка — у кода в venues/<биржа>/errors.rs): Binance
//!     −4023, KuCoin 110003, Bybit 131095, OKX 58201 / 58256 / 58257 / 58258, XT WITHDRAW_012 / 013 / 020 / 021; у
//!     других операций — прежний смысл; у каждого — человеческая причина;
//! (2) коды прав у ВЫВОДА → AccountRestricted (MEXC 60005, Bybit 131004 / 131090–131092, KuCoin 411100 / 400700 /
//!     200009, Gate ACCOUNT_LOCKED, OKX 58004); у других операций — InsufficientPermissions, как раньше;
//! (3) ядро: WithdrawLimit — отказ вывода (без повтора по правилу, не ошибка подряд, вывод не создан); площадка на
//!     паузе с отметкой WithdrawLimit «только человек» (нет автоснятия, новый день и «Сбросить лимит дня» её не снимают,
//!     поверх «исход неизвестен» — возвращается после выяснения); монета не закрыта; журнал core.withdraw_limit; снимает
//!     «Снять паузу»;
//! (4) маршрут новой роли чата «Аккаунты»: venue.account_problem и venue.withdraw_limit — в notify.accounts_chat
//!     (вместо «Общего», если задан); без кнопок.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::*::errors, arb::notify (reason, roles, routes, press), arb::core (rules_apply,
//! auto_pause, outcome_open, day_roll, withdraw_block, risk_count, op_error), arb::gateway::rules. Моков нет.

mod usdt_return_common;

use arb::core::auto_pause::{self as ap, Kind};
use arb::core::op_error::OpError;
use arb::core::risk::Risk;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::errors::{ErrorCode as C, GatewayError};
use arb::notify::roles::Role;
use arb::notify::routes::{Tag, extra, not_general};
use arb::settings::value::Scope;
use arb::venues;
use chrono::{TimeZone, Utc};
use serde_json::json;
use usdt_return_common::{dir, entries, start, start_with};

/// Проверка одного разбора: ожидаемый код и человеческая причина (известный код — без «ответа биржи как есть»).
fn check(e: &GatewayError, want: C, human: &str) {
    assert_eq!(e.code, want, "{}", e.message);
    let r = arb::notify::reason::human(&e.message);
    assert!(
        r.answer.is_none(),
        "код не в словаре: {} → {r:?}",
        e.message
    );
    assert!(r.text.contains(human), "{} → {r:?}", e.message);
}

#[test]
fn limit_codes_from_tables() {
    use venues::{binance, bybit, kucoin, okx, xt};
    let msg = "Within 24 hours, the withdrawal exceeds the maximum amount.";
    let raw = json!({"code": -4023, "msg": msg}).to_string();
    let w = binance::errors::Op::Withdraw;
    check(
        &binance::errors::from_code(-4023, msg, w, &raw),
        C::WithdrawLimit,
        "суточный лимит",
    );
    let o = binance::errors::Op::Other;
    assert_ne!(
        binance::errors::from_code(-4023, msg, o, &raw).code,
        C::WithdrawLimit
    );

    let msg = "Withdrawal limit exceeded. The withdrawal request hit the limit.";
    let w = kucoin::errors::Op::Withdraw;
    check(
        &kucoin::errors::from_code("110003", msg, w, "{}"),
        C::WithdrawLimit,
        "суточный лимит",
    );

    let msg = "Withdrawl amount exceeds the 24 hour platform limit";
    let w = bybit::errors::Op::Withdraw;
    check(
        &bybit::errors::from_code(131095, msg, w, "{}"),
        C::WithdrawLimit,
        "суточный лимит",
    );

    let w = okx::errors::Op::Withdraw;
    let okx_limits = [
        (
            "58201",
            "Withdrawal amount exceeds daily withdrawal limit.",
            "суточный лимит",
        ),
        (
            "58256",
            "You can withdraw stablecoins valued at up to {dailyLimit} {symbol} every 24 hours.",
            "суточный лимит",
        ),
        (
            "58257",
            "You can withdraw stablecoins valued at up to {monthlyLimit} {symbol} every month.",
            "месячный лимит",
        ),
        (
            "58258",
            "You can withdraw crypto valued at up to {dailyLimit} {symbol} every 24 hours.",
            "суточный лимит",
        ),
    ];
    for (code, msg, h) in okx_limits {
        check(
            &okx::errors::from_code(code, msg, w, "{}"),
            C::WithdrawLimit,
            h,
        );
        let o = okx::errors::Op::Other;
        assert_ne!(
            okx::errors::from_code(code, msg, o, "{}").code,
            C::WithdrawLimit
        );
    }

    let w = xt::errors::Op::Withdraw;
    for mc in [
        "WITHDRAW_012",
        "WITHDRAW_013",
        "WITHDRAW_020",
        "WITHDRAW_021",
    ] {
        check(
            &xt::errors::from_code(mc, "", w, "{}"),
            C::WithdrawLimit,
            "суточный лимит",
        );
    }
}

#[test]
fn permission_codes_at_withdraw_are_account() {
    use venues::{bybit, gate, kucoin, mexc, okx};
    let msg = "your account is abnormal";
    check(
        &mexc::errors::from_code(60005, msg, mexc::errors::Op::Withdraw, "{}"),
        C::AccountRestricted,
        "необычным",
    );
    assert_eq!(
        mexc::errors::from_code(60005, msg, mexc::errors::Op::Trade, "{}").code,
        C::InsufficientPermissions
    );

    let by = [
        (131004, "KYC needed", "KYC"),
        (131090, "User withdraw has been banned", "запрещён"),
        (
            131091,
            "Blocked login status does not allow withdrawals",
            "заблокирован",
        ),
        (131092, "User status is abnormal", "необычным"),
    ];
    for (code, msg, h) in by {
        let w = bybit::errors::Op::Withdraw;
        check(
            &bybit::errors::from_code(code, msg, w, "{}"),
            C::AccountRestricted,
            h,
        );
        let t = bybit::errors::Op::Trade;
        assert_eq!(
            bybit::errors::from_code(code, msg, t, "{}").code,
            C::InsufficientPermissions
        );
    }

    let ku = [
        ("411100", "User is frozen", "заморожен"),
        (
            "400700",
            "Transaction restricted, there's a risk problem in your account",
            "риск",
        ),
        (
            "200009",
            "Please complete the KYC verification before you trade XX",
            "KYC",
        ),
    ];
    for (code, msg, h) in ku {
        let w = kucoin::errors::Op::Withdraw;
        check(
            &kucoin::errors::from_code(code, msg, w, "{}"),
            C::AccountRestricted,
            h,
        );
        let o = kucoin::errors::Op::Other;
        assert_eq!(
            kucoin::errors::from_code(code, msg, o, "{}").code,
            C::InsufficientPermissions
        );
    }

    let w = gate::errors::Op::Withdraw;
    let e = gate::errors::from_label(403, "ACCOUNT_LOCKED", "Account is locked", w, "{}");
    check(&e, C::AccountRestricted, "заблокирован");
    let t = gate::errors::Op::Trade;
    let e = gate::errors::from_label(403, "ACCOUNT_LOCKED", "Account is locked", t, "{}");
    assert_eq!(e.code, C::InsufficientPermissions);

    let w = okx::errors::Op::Withdraw;
    check(
        &okx::errors::from_code("58004", "Account blocked.", w, "{}"),
        C::AccountRestricted,
        "заблокирован",
    );
    let o = okx::errors::Op::Other;
    assert_eq!(
        okx::errors::from_code("58004", "Account blocked.", o, "{}").code,
        C::InsufficientPermissions
    );
}

#[test]
fn core_treats_limit_as_refusal() {
    let e = GatewayError::new(C::WithdrawLimit, Some(VenueId("binance".into())), "x");
    assert!(arb::core::withdraw_refused::refusal(Some(C::WithdrawLimit)));
    assert!(arb::gateway::rules::withdraw_refusal(C::WithdrawLimit));
    assert!(arb::core::auto_pause::no_pause(C::WithdrawLimit));
    assert!(!arb::core::risk_count::counts(&e));
    assert!(OpError::from(&e).withdraw_unsent());
    let codes = arb::api::rules_codes::codes();
    let m = codes.iter().find(|c| c.code == C::WithdrawLimit).unwrap();
    assert!(m.no_retry);
    assert!(m.inactive["StopVenue"].contains("ставится сама"));
}

fn t0() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 11, 10, 0, 0).unwrap()
}

#[test]
fn limit_pause_is_human_only() {
    let v = VenueId("binance".into());
    let mut r = Risk::default();
    assert!(ap::pause_venue(
        &mut r,
        &v,
        "лимит".into(),
        Kind::WithdrawLimit,
        t0()
    ));
    assert!(ap::human_only(&r, &v));
    let m = r.venue_auto[&v].clone();
    assert!(!ap::due(&m, t0() + chrono::Duration::days(30), Some(0)));
    assert!(!ap::lift_venue(&mut r, &v, &m), "автоснятия нет");
    // Ошибки подряд и правило поверх — отметка остаётся WithdrawLimit.
    ap::pause_venue(&mut r, &v, "3 ошибок подряд".into(), Kind::Errors, t0());
    ap::pause_venue(&mut r, &v, "правило шлюза №4".into(), Kind::Rule, t0());
    assert_eq!(r.venue_auto[&v].kind, Kind::WithdrawLimit);

    // Вместе с паузой направления по убытку дня (решение 258: отдельно, Risk.pair_paused) — своя отметка; новый день
    // снимает паузу направления, а паузу площадки по лимиту вывода — нет.
    let mut r = Risk::default();
    r.pair_paused
        .insert("binance:cex_dex".into(), "убыток дня".into());
    ap::pause_venue(&mut r, &v, "лимит".into(), Kind::WithdrawLimit, t0());
    assert_eq!(r.venue_auto[&v].kind, Kind::WithdrawLimit);
    let lift = arb::core::day_roll::new_day(&mut r);
    assert!(r.venue_paused.contains_key(&v), "{lift:?}");
    assert!(r.pair_paused.is_empty());
}

#[test]
fn limit_pause_returns_after_outcome() {
    use arb::core::outcome_open as open;
    let v = VenueId("okx".into());
    let mut r = Risk::default();
    // Лимит, потом «исход неизвестен» — лимит запоминается прежней паузой и возвращается.
    ap::pause_venue(&mut r, &v, "лимит".into(), Kind::WithdrawLimit, t0());
    let prior = open::prior(&r, &v);
    assert!(prior.is_some());
    ap::pause_venue(&mut r, &v, "исход".into(), Kind::Outcome, t0());
    open::register(&mut r, &v, Some("k1"), prior);
    assert_eq!(open::resolved(&mut r, &v, "k1"), open::Lift::Lifted);
    assert_eq!(r.venue_auto[&v].kind, Kind::WithdrawLimit);
    assert_eq!(r.venue_paused[&v], "лимит");

    // «Исход неизвестен», потом лимит — тоже вернётся после выяснения.
    let mut r = Risk::default();
    let prior = open::prior(&r, &v);
    ap::pause_venue(&mut r, &v, "исход".into(), Kind::Outcome, t0());
    open::register(&mut r, &v, Some("k2"), prior);
    ap::pause_venue(&mut r, &v, "лимит".into(), Kind::WithdrawLimit, t0());
    assert_eq!(r.venue_auto[&v].kind, Kind::Outcome);
    assert_eq!(open::resolved(&mut r, &v, "k2"), open::Lift::Lifted);
    assert_eq!(r.venue_auto[&v].kind, Kind::WithdrawLimit);
    assert_eq!(r.venue_paused[&v], "лимит");
}

/// Отказ вывода Binance −4023 (суточный лимит) в сделке: площадка на паузе (только человек), монета не закрыта, журнал;
/// повторный отказ — пауза уже стояла; «Снять паузу» снимает.
#[tokio::test]
async fn limit_refusal_pauses_venue_keeps_coin() {
    use venues::binance::errors::{Op, from_code};
    let d = dir("t85lim");
    let (core, j) = start(&d, None).await;
    let binance = VenueId("binance".into());
    let coin = Asset {
        ticker: "CYBER".into(),
        bsc_contract: Some(alloy_primitives::address!(
            "0x0000000000000000000000000000000000085a01"
        )),
    };
    let msg = "Within 24 hours, the withdrawal exceeds the maximum amount.";
    let raw = json!({"code": -4023, "msg": msg}).to_string();
    let g = from_code(-4023, msg, Op::Withdraw, &raw);
    let deal = "20261111100000000CYBER";
    let next = arb::core::rules_apply::on_error(&core, "Вывод", &binance, Some(deal), &g, 1).await;
    assert_eq!(next, arb::core::rules_apply::Next::Stop);
    let r = core.control();
    assert!(r.venue_paused[&binance].contains("лимит вывода исчерпан"));
    assert_eq!(r.venue_auto[&binance].kind, Kind::WithdrawLimit);
    assert!(!r.errors_in_row.contains_key(&binance), "не ошибка подряд");
    // Сделка дальше — как при отказе вывода: монета не закрывается.
    arb::core::withdraw_block::close(&core, deal, &coin, &binance, &OpError::from(&g)).await;
    let dir = Direction {
        asset: coin,
        buy_on: binance.clone(),
        sell_on: VenueId("bsc_dex".into()),
    };
    assert!(arb::core::withdraw_block::check(&core, &dir).is_ok());
    assert!(
        entries(&j, "core.withdraw_block", "").await.is_empty(),
        "ни закрытия монеты, ни «проблемы в аккаунте»"
    );
    let logs = entries(
        &j,
        "core.withdraw_limit",
        "площадка на паузе до ручного снятия",
    )
    .await;
    assert_eq!(logs.len(), 1);
    // Повтор — пауза уже стояла (алерт не повторяется).
    arb::core::rules_apply::on_error(&core, "Вывод", &binance, Some(deal), &g, 1).await;
    let again = entries(&j, "core.withdraw_limit", "пауза уже стояла").await;
    assert_eq!(again.len(), 1);
    core.resume_venue(&binance).await;
    let r = core.control();
    assert!(!r.venue_paused.contains_key(&binance));
    assert!(!r.venue_auto.contains_key(&binance));
}

#[test]
fn accounts_role_routes() {
    assert_eq!(extra("venue.withdraw_limit", Tag::Plain), &[Role::Accounts]);
    assert_eq!(
        extra("venue.account_problem", Tag::Plain),
        &[Role::Accounts]
    );
    assert_eq!(Role::Accounts.chat_key(), "notify.accounts_chat");
    assert_eq!(Role::Accounts.token_secret(), "telegram.accounts_token");
    assert!(Role::Accounts.users_key().is_none(), "без кнопок");
    assert!(!arb::notify::press::accepts(Role::Accounts, "sell_now"));
    assert_eq!(Role::from_code("accounts"), Some(Role::Accounts));
    assert!(
        arb::secrets::KNOWN
            .iter()
            .any(|(n, _, _)| *n == "telegram.accounts_token")
    );
}

/// Чат «Аккаунты» задан — алерты аккаунта в «Общий» не идут; не задан — в «Общий», как раньше.
#[tokio::test]
async fn accounts_chat_replaces_general() {
    let d = dir("t85none");
    let (core, _j) = start(&d, None).await;
    let s = &core.inner.deps.settings;
    assert_eq!(not_general(s, "venue.withdraw_limit"), None);
    assert_eq!(not_general(s, "venue.account_problem"), None);
    let d = dir("t85chat");
    let set = vec![(
        "notify.accounts_chat",
        Scope::Global,
        json!("-1001234567890"),
    )];
    let (core, _j) = start_with(&d, None, set).await;
    let s = &core.inner.deps.settings;
    assert_eq!(not_general(s, "venue.withdraw_limit"), Some(Role::Accounts));
    assert_eq!(
        not_general(s, "venue.account_problem"),
        Some(Role::Accounts)
    );
    assert_eq!(not_general(s, "exec.withdraw_rejected"), None);
}
