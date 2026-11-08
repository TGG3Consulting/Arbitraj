//! Файл: tests/core_auto_resume.rs
//! Что проверяет: решение 225 — «пауза-плей автоматически — только когда на паузу поставил сам софт» (чистая логика
//! core/auto_pause.rs и core/day_roll.rs, без ядра):
//! 1) отметка софтовой паузы площадки: новая пауза — вид; сильнее заменяет слабую (время первой постановки остаётся);
//!    пауза без отметки (убыток дня, сверка, человек) отметку не получает; «исход неизвестен» ставит «только человек»
//!    поверх любой паузы, и её причину другие паузы не перезаписывают;
//! 2) когда проверять подключение: ошибки подряд — после ожидания (не задано — никогда), правило — сразу, «исход
//!    неизвестен» — никогда;
//! 3) снятие — только если отметка не сменилась (человек снял / стала сильнее — нет); «исход неизвестен» — никогда;
//! 4) «Стоп» от софта: режим до него возвращается, если отметка та же и система в «Стоп»; смена режима человеком
//!    (отметки нет) — ничего; «Стоп» поверх «Стоп» отметки не ставит;
//! 5) коды без паузы (WithdrawRejected, InvalidAddress, WithdrawLocked) — паузы нет и ошибкой подряд не считаются;
//! 6) новый день: паузы по убытку дня (биржа, циклы DEX) и «Стоп» по общему лимиту сняты, «исход неизвестен» остаётся;
//!    до полуночи UTC — точное ожидание.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::auto_pause, arb::core::day_roll, arb::core::risk, arb::core::risk_count.

use chrono::{DateTime, Duration, TimeZone, Utc};

use arb::core::auto_pause::{self as ap, Kind, ModeKind};
use arb::core::day_roll::{DayLift, new_day, until_midnight};
use arb::core::risk::{Mode, Risk};
use arb::core::risk_count;
use arb::core::types::VenueId;
use arb::gateway::errors::{ErrorCode, GatewayError};

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 8, 10, 0, 0).unwrap()
}

fn mexc() -> VenueId {
    VenueId("mexc".into())
}

#[test]
fn mark_new_stronger_and_foreign_pause() {
    let mut r = Risk::default();
    let v = mexc();
    assert!(ap::pause_venue(
        &mut r,
        &v,
        "3 ошибок подряд".into(),
        Kind::Errors,
        t0()
    ));
    assert_eq!(r.venue_auto[&v].kind, Kind::Errors);
    // Правило сильнее ошибок: вид Rule, время первой постановки сохраняется.
    let later = t0() + Duration::seconds(30);
    assert!(ap::pause_venue(
        &mut r,
        &v,
        "правило шлюза №4".into(),
        Kind::Rule,
        later
    ));
    assert_eq!(r.venue_auto[&v].kind, Kind::Rule);
    assert_eq!(r.venue_auto[&v].since, t0());
    // Ошибки поверх правила — слабее: вид не меняется, не «новое».
    assert!(!ap::pause_venue(
        &mut r,
        &v,
        "4 ошибок подряд".into(),
        Kind::Errors,
        later
    ));
    assert_eq!(r.venue_auto[&v].kind, Kind::Rule);

    // Пауза без отметки (убыток дня) — ошибки отметку не дают: её снимет своя логика или человек.
    let mut r = Risk::default();
    r.venue_paused
        .insert(v.clone(), "MEXC: убыток за день".into());
    ap::pause_venue(&mut r, &v, "3 ошибок подряд".into(), Kind::Errors, t0());
    assert!(!r.venue_auto.contains_key(&v));
}

#[test]
fn outcome_is_human_only_over_any_pause() {
    let mut r = Risk::default();
    let v = mexc();
    r.venue_paused
        .insert(v.clone(), "MEXC: убыток за день".into());
    let why = "Вывод: исход транзакции неизвестен — нужна ручная проверка";
    assert!(ap::pause_venue(&mut r, &v, why.into(), Kind::Outcome, t0()));
    assert!(ap::human_only(&r, &v));
    assert_eq!(r.venue_paused[&v], why);
    // Повтор «исход неизвестен» — не новое (одно уведомление на смену состояния).
    assert!(!ap::pause_venue(
        &mut r,
        &v,
        why.into(),
        Kind::Outcome,
        t0()
    ));
    // Ошибки поверх — причину не перезаписывают, отметка остаётся.
    ap::pause_venue(&mut r, &v, "5 ошибок подряд".into(), Kind::Errors, t0());
    assert_eq!(r.venue_paused[&v], why);
    assert!(ap::human_only(&r, &v));
    // И снять автоматикой нельзя.
    let m = r.venue_auto[&v].clone();
    assert!(!ap::due(&m, t0() + Duration::days(1), Some(0)));
    assert!(!ap::lift_venue(&mut r, &v, &m));
    assert!(r.venue_paused.contains_key(&v));
}

#[test]
fn due_by_kind() {
    let m = |kind| ap::VenueMark { kind, since: t0() };
    let at = |s| t0() + Duration::seconds(s);
    assert!(!ap::due(&m(Kind::Errors), at(299), Some(300)));
    assert!(ap::due(&m(Kind::Errors), at(300), Some(300)));
    assert!(
        !ap::due(&m(Kind::Errors), at(100_000), None),
        "ожидание не задано — снимает человек"
    );
    assert!(ap::due(&m(Kind::Rule), at(0), None));
    assert!(!ap::due(&m(Kind::Outcome), at(100_000), Some(0)));
}

#[test]
fn lift_only_same_mark() {
    let v = mexc();
    let mut r = Risk::default();
    ap::pause_venue(&mut r, &v, "правило шлюза №4".into(), Kind::Rule, t0());
    r.errors_in_row.insert(v.clone(), 3);
    let seen = r.venue_auto[&v].clone();
    assert!(ap::lift_venue(&mut r, &v, &seen));
    assert!(!r.venue_paused.contains_key(&v));
    assert!(!r.errors_in_row.contains_key(&v));
    assert!(!r.venue_auto.contains_key(&v));

    // Человек снял паузу (отметка снята) — проверка, начатая раньше, ничего не делает.
    let mut r = Risk::default();
    ap::pause_venue(&mut r, &v, "3 ошибок подряд".into(), Kind::Errors, t0());
    let seen = r.venue_auto[&v].clone();
    r.venue_auto.remove(&v);
    assert!(!ap::lift_venue(&mut r, &v, &seen));
    assert!(r.venue_paused.contains_key(&v));

    // За время проверки пауза стала сильнее — тоже ничего.
    let mut r = Risk::default();
    ap::pause_venue(&mut r, &v, "3 ошибок подряд".into(), Kind::Errors, t0());
    let seen = r.venue_auto[&v].clone();
    ap::pause_venue(&mut r, &v, "правило шлюза №4".into(), Kind::Rule, t0());
    assert!(!ap::lift_venue(&mut r, &v, &seen));
}

#[test]
fn mode_restore_only_own_stop() {
    let mut r = Risk::default();
    ap::mark_mode(
        &mut r,
        ModeKind::Rule { venue: mexc() },
        Mode::Running,
        None,
        t0(),
    );
    r.mode = Mode::Stopped;
    let m = r.mode_auto.clone().unwrap();
    assert_eq!(
        ap::restore_mode(&mut r, &m, "снято".into()),
        Some(Mode::Running)
    );
    assert_eq!(r.mode, Mode::Running);
    assert_eq!(r.reason.as_deref(), Some("снято"));
    assert!(r.mode_auto.is_none());

    // «Пауза» человека до стопа — возвращается «Пауза» с его причиной.
    let mut r = Risk::default();
    ap::mark_mode(
        &mut r,
        ModeKind::DailyTotal,
        Mode::Paused,
        Some("команда: интерфейс".into()),
        t0(),
    );
    r.mode = Mode::Stopped;
    r.daily_stop = true;
    let m = r.mode_auto.clone().unwrap();
    assert_eq!(
        ap::restore_mode(&mut r, &m, "снято".into()),
        Some(Mode::Paused)
    );
    assert_eq!(r.reason.as_deref(), Some("команда: интерфейс"));
    assert!(!r.daily_stop);

    // Человек сменил режим (отметки нет) — ничего не возвращается.
    let mut r = Risk::default();
    ap::mark_mode(&mut r, ModeKind::DailyTotal, Mode::Running, None, t0());
    let m = r.mode_auto.clone().unwrap();
    r.mode_auto = None;
    r.mode = Mode::Stopped;
    assert_eq!(ap::restore_mode(&mut r, &m, "снято".into()), None);
    assert_eq!(r.mode, Mode::Stopped);

    // «Стоп» поверх «Стоп» — отметки нет (возвращать нечего).
    let mut r = Risk::default();
    ap::mark_mode(
        &mut r,
        ModeKind::Rule { venue: mexc() },
        Mode::Stopped,
        None,
        t0(),
    );
    assert!(r.mode_auto.is_none());
}

#[test]
fn withdraw_codes_never_pause() {
    for code in [
        ErrorCode::WithdrawRejected,
        ErrorCode::InvalidAddress,
        ErrorCode::WithdrawLocked,
    ] {
        assert!(ap::no_pause(code));
        assert!(!risk_count::counts(&GatewayError::new(
            code,
            Some(mexc()),
            "отказ"
        )));
    }
    assert!(!ap::no_pause(ErrorCode::InvalidCredentials));
    assert!(risk_count::counts(&GatewayError::new(
        ErrorCode::Timeout,
        Some(mexc()),
        "нет ответа"
    )));
}

#[test]
fn new_day_lifts_loss_pauses_keeps_outcome() {
    let mut r = Risk::default();
    let (bx, dex, gate) = (
        VenueId("bingx".into()),
        "bsc_dex".to_string(),
        VenueId("gate".into()),
    );
    r.venue_paused
        .insert(bx.clone(), "BingX: убыток за день".into());
    r.loss_paused.insert("bingx".into());
    r.cycle_paused
        .insert(dex.clone(), "DEX ↔ DEX: убыток за день".into());
    r.loss_paused.insert(dex.clone());
    r.venue_paused
        .insert(gate.clone(), "Gate: убыток за день".into());
    r.loss_paused.insert("gate".into());
    ap::pause_venue(
        &mut r,
        &gate,
        "Вывод: исход неизвестен".into(),
        Kind::Outcome,
        t0(),
    );
    // Человек поставил «Пауза» до лимита дня — вернётся «Пауза».
    ap::mark_mode(
        &mut r,
        ModeKind::DailyTotal,
        Mode::Paused,
        Some("команда: интерфейс".into()),
        t0(),
    );
    r.mode = Mode::Stopped;
    r.daily_stop = true;
    let l = new_day(&mut r);
    assert_eq!(
        l,
        DayLift {
            venues: vec!["bingx".into()],
            cycles: vec![dex.clone()],
            kept: vec!["gate".into()],
            mode: Some(Mode::Paused),
        }
    );
    assert!(!r.venue_paused.contains_key(&bx));
    assert!(r.cycle_paused.is_empty());
    assert!(r.venue_paused.contains_key(&gate));
    assert!(r.loss_paused.is_empty());
    assert_eq!(r.mode, Mode::Paused);
    assert!(!r.daily_stop);

    // «Стоп» по правилу (не лимит дня) новый день не трогает.
    let mut r = Risk::default();
    ap::mark_mode(
        &mut r,
        ModeKind::Rule { venue: bx },
        Mode::Running,
        None,
        t0(),
    );
    r.mode = Mode::Stopped;
    assert_eq!(new_day(&mut r).mode, None);
    assert_eq!(r.mode, Mode::Stopped);
}

#[test]
fn midnight_wait() {
    let now = Utc.with_ymd_and_hms(2026, 11, 8, 23, 59, 30).unwrap();
    assert_eq!(
        until_midnight(now),
        Some(std::time::Duration::from_secs(30))
    );
    assert_eq!(
        until_midnight(t0()),
        Some(std::time::Duration::from_secs(14 * 3600))
    );
}
