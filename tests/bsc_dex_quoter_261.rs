//! Файл: tests/bsc_dex_quoter_261.rs
//! Что делает: решение 261 (Тигран 12.11.2026) на реально записанных данных:
//! - часть 1, сверка перед сделкой односторонняя — цифры живого случая 05.11 (сделка 20261105131254836GAIB, расчёт
//!   42559024903490846447 против квотера 42489232805231828113, допуск 0.1%): квотер меньше расчёта — стоп (покупка,
//!   продажа без признака выхода); квотер больше расчёта на те же 0.164% — сделка идёт, только строка «в нашу сторону»
//!   (не поправка: ожидаемое и минимум — по нашему расчёту); продажа выхода (решение 177) — как была;
//! - часть 2, сумма сверки — min(наш максимум money.max_per_trade_usd среди направлений с DEX, ликвидность стороны);
//! - часть 2, блок сверки по позиции пула и записанным событиям пула AAVE V3 0x8e3ecc0b… (tests/data/ws_logs_2026-10-26:
//!   блоки 129913959 logIndex 211 и 129914008 logIndex 1098) — «не сверено (блок)» или блок сверки;
//! - часть 2, пауза и автоматическое снятие стороны пула и текст алерта (живой пул AAVE V3, запись 2026-10-25).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{check, exec, quote}, arb::settings.

mod depth_common;
mod native_common;

use std::str::FromStr;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::Side;
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::quoter_amount::{MAX, max_of, pick};
use arb::venues::bsc_dex::check::quoter_block::{NOT_CHECKED, settle};
use arb::venues::bsc_dex::check::quoter_compare::{Cmp, PAUSED, judged, no_block};
use arb::venues::bsc_dex::check::quoter_pause::{Change, apply, text};
use arb::venues::bsc_dex::check::quoter_sides::QuoterSides;
use arb::venues::bsc_dex::check::quoter_verify::Verdict;
use arb::venues::bsc_dex::exec::{precheck, quoter_favour};
use arb::venues::bsc_dex::quote::quoter::{self, Judged};

use depth_common::{aave_v3, rec};

/// Живой случай 05.11: расчёт и квотер (USDT, 18 знаков).
const LOCAL: &str = "42559024903490846447";
const REMOTE: &str = "42489232805231828113";
/// Пул AAVE V3 (Uniswap V3) в записи WebSocket-событий 2026-10-26.
const AAVE: &str = "0x8e3ecc0b261f1a4db62321090575eb299844f077";
const WS: &str = "tests/data/ws_logs_2026-10-26/responses.json";

fn u(s: &str) -> U256 {
    U256::from_str(s).unwrap()
}

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Окружение с допуском сверки 0.1% (как в живом случае).
async fn ctx(tag: &str) -> arb::venues::bsc_dex::ctx::Ctx {
    let c = native_common::ctx(tag).await;
    c.settings
        .set(
            "dex.quoter_max_diff_pct",
            Scope::Venue("bsc_dex".into()),
            Some(json!("0.1")),
            "тест",
        )
        .await
        .unwrap();
    c
}

#[tokio::test]
async fn pre_trade_is_one_sided() {
    let c = ctx("t261-one").await;
    let (e, _, _) = aave_v3(&rec());
    // Квотер МЕНЬШЕ расчёта на 0.164% > 0.1% — против нас: стоп у покупки и у продажи без признака выхода.
    let against = quoter::judge(&c, u(LOCAL), u(REMOTE), Decimal::ZERO).unwrap();
    assert!(!against.favours_us());
    let err = precheck::verdict(&c, &e, against.clone(), false).unwrap_err();
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
    // Продажа выхода (решение 177) — как была: поправка по квотеру, не favour.
    let fix = precheck::verdict(&c, &e, against, true).unwrap().unwrap();
    assert!(!fix.favour);
    // Квотер БОЛЬШЕ расчёта на те же цифры — в нашу сторону: сделка идёт, строка в хронологию.
    let ours = quoter::judge(&c, u(REMOTE), u(LOCAL), Decimal::ZERO).unwrap();
    let Judged::Diff { pct, .. } = &ours else {
        panic!("ждали расхождение: {ours:?}");
    };
    assert!(*pct > d("0.1"));
    assert!(ours.favours_us());
    let fav = precheck::verdict(&c, &e, ours.clone(), false)
        .unwrap()
        .unwrap();
    assert!(fav.favour);
    assert!(
        fav.text()
            .contains("в нашу сторону, сделка идёт (решение 261)")
    );
    assert!(fav.text().contains(LOCAL) && fav.text().contains(REMOTE));
    // Поправки продажи выхода нет (минимум — по нашему расчёту), только строка.
    let (exit, lines) = quoter_favour::split(vec![(0, fav)]);
    assert!(exit.is_empty());
    assert_eq!(lines.len(), 1);
    assert_eq!(
        quoter_favour::join(Some("a".into()), Some("b".into())).as_deref(),
        Some("a; b")
    );
    // Продажа выхода с квотером больше расчёта — решение 177 без изменений (поправка по квотеру).
    let exit_fix = precheck::verdict(&c, &e, ours, true).unwrap().unwrap();
    assert!(!exit_fix.favour);
    // В допуске — ни стопа, ни строки.
    let within = quoter::judge(&c, u(LOCAL), u(LOCAL), Decimal::ZERO).unwrap();
    assert_eq!(within, Judged::Within);
    assert!(precheck::verdict(&c, &e, within, false).unwrap().is_none());
}

#[tokio::test]
async fn amount_is_min_of_max_and_liquidity() {
    // Ликвидность меньше максимума — по ликвидности; больше — по максимуму.
    assert_eq!(pick(d("500"), d("120.5")).usd, d("120.5"));
    assert_eq!(pick(d("500"), d("9000")).usd, d("500"));
    let c = native_common::ctx("t261-max").await;
    let s = &c.settings;
    // Не задан ни у одного направления — «не задана настройка» с её именем.
    let e = max_of(s, "bsc_dex", &["bingx".into()], true).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotConfigured);
    assert_eq!(e.setting.as_deref(), Some(MAX));
    // Решение 242: пара сильнее площадки, площадка сильнее системы; берём наибольшее среди направлений с DEX.
    for (scope, v) in [
        (Scope::VenueDir("bingx:cex_dex".into()), json!(50)),
        (Scope::Venue("bsc_dex".into()), json!(80)),
        (Scope::VenueDir("dex_dex".into()), json!(120)),
    ] {
        s.set(MAX, scope, Some(v), "тест").await.unwrap();
    }
    // bingx → DEX 50, DEX → bingx — площадка покупки DEX 80; DEX ↔ DEX не привязан — не считается.
    assert_eq!(
        max_of(s, "bsc_dex", &["bingx".into()], false).unwrap(),
        d("80")
    );
    // DEX ↔ DEX привязан — 120.
    assert_eq!(
        max_of(s, "bsc_dex", &["bingx".into()], true).unwrap(),
        d("120")
    );
}

/// Позиции (блок, logIndex) записанных событий пула AAVE и наибольший блок записи (узел до него дошёл).
fn aave_logs() -> (Vec<(u64, u64)>, u64) {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(WS).unwrap()).unwrap();
    let hex =
        |x: &Value| u64::from_str_radix(x.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    let frames = v["frames"].as_array().unwrap();
    let head = frames.iter().map(|f| hex(&f["blockNumber"])).max().unwrap();
    let mine = frames
        .iter()
        .filter(|f| f["address"] == AAVE && f["removed"] == false)
        .map(|f| (hex(&f["blockNumber"]), hex(&f["logIndex"])))
        .collect();
    (mine, head)
}

#[test]
fn block_of_check_from_recorded_logs() {
    let (logs, head) = aave_logs();
    assert_eq!(logs, vec![(129913959, 211), (129914008, 1098)]);
    // Память учла первое событие, а у пула есть следующее — «не сверено (блок)», не ошибка математики.
    let why = settle(129913959, Some(211), false, &logs, head).unwrap_err();
    assert!(why.contains("ещё не догнало сеть"), "{why}");
    assert!(matches!(no_block(&why), Cmp::NoBlock(t) if t.starts_with(NOT_CHECKED)));
    // Учтено последнее событие — состояние равно сети на голове: сверка на голове.
    assert_eq!(settle(129914008, Some(1098), false, &logs, head), Ok(head));
    // Позиция — середина блока (logIndex меньше записанного события того же блока) — не на конец блока.
    assert!(settle(129914008, Some(1000), false, &logs, head).is_err());
    // Прочитано на конце блока целиком (logIndex None) — события этого блока уже внутри.
    assert_eq!(settle(129914008, None, false, &logs, head), Ok(head));
    // Узел HTTP позади состояния пула и неизвестный блок — не сверено.
    assert!(settle(head + 1, None, false, &[], head).is_err());
    assert!(settle(0, None, false, &[], head).is_err());
    // Пул, перечитываемый на каждом блоке: только блок чтения целиком.
    assert_eq!(settle(129914008, None, true, &[], head), Ok(129914008));
    assert!(settle(129914008, Some(5), true, &[], head).is_err());
}

#[tokio::test]
async fn pause_and_auto_resume_with_one_alert_each() {
    let c = ctx("t261-pause").await;
    let (e, _, _) = aave_v3(&rec());
    let block = 129914008;
    let Cmp::Pause { why, nums, .. } = judged(
        quoter::judge(&c, u(LOCAL), u(REMOTE), Decimal::ZERO).unwrap(),
        block,
    ) else {
        panic!("ждали паузу");
    };
    assert!(why.starts_with(PAUSED));
    let pause = Verdict::Pause {
        why,
        block,
        usd: d("500"),
        nums,
    };
    let mut q = QuoterSides::default();
    q.set(Side::Buy, Some(Ok(())));
    // Ушла на паузу — одно событие; сторона исключена (итог — Err), другая сторона не тронута.
    assert_eq!(apply(&mut q, Side::Buy, &pause), Some(Change::Paused));
    assert!(q.paused(Side::Buy) && matches!(q.buy, Some(Err(_))));
    assert!(q.sell.is_none() && !q.paused(Side::Sell));
    // Повтор того же расхождения через dex.quoter_check_s — без нового алерта.
    assert_eq!(apply(&mut q, Side::Buy, &pause), None);
    // «Не сверено (блок)» — пауза и итог прежние, причина — в заметке.
    let nb = Verdict::NoBlock(format!("{NOT_CHECKED}: узел HTTP на блоке 1"));
    assert_eq!(apply(&mut q, Side::Buy, &nb), None);
    assert!(q.paused(Side::Buy) && matches!(q.buy, Some(Err(_))) && q.buy_note.is_some());
    // Иная причина — пауза держится (снимает только прошедшая сверка).
    let fail = Verdict::Fail(("пул пуст".into(), None));
    assert_eq!(apply(&mut q, Side::Buy, &fail), None);
    assert!(q.paused(Side::Buy));
    // Сверка на одном блоке прошла — пауза снята сама, одно событие.
    let ok = Verdict::Ok { block: block + 1 };
    assert_eq!(apply(&mut q, Side::Buy, &ok), Some(Change::Resumed));
    assert!(!q.paused(Side::Buy) && q.buy == Some(Ok(())) && q.buy_note.is_none());
    // Текст алерта: громко, пул, DEX, монета, сторона, сумма, наш расчёт, квотер, %, блок.
    let t = text(&e, Side::Buy, &pause, Change::Paused, Some(18));
    for part in [
        "🚨",
        "!!!",
        e.family.title,
        e.ticker.as_str(),
        "покупка",
        "500 USDT",
        LOCAL,
        REMOTE,
        "0.1643%",
        "Блок: 129914008",
    ] {
        assert!(t.contains(part), "нет «{part}» в {t}");
    }
    let r = text(&e, Side::Buy, &ok, Change::Resumed, Some(18));
    assert!(r.contains("Пауза снята") && r.contains("129914009"));
}

/// Ответы своего узла BSC 12.11.2026 (tests/data/node_limits_2026-11-12): отказ eth_getLogs по диапазону — не «нет
/// состояния блока» (сверка даёт «не сверено (блок)» с текстом узла); нет состояния старше H−127 — StaleData
/// (errors.rs from_rpc), у сверки — тоже «не сверено (блок)», не ошибка математики.
#[test]
fn node_limits_recorded_answers() {
    use arb::venues::bsc_dex::errors::{Op, from_rpc};
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/node_limits_2026-11-12/responses.json").unwrap(),
    )
    .unwrap();
    let err = |x: &Value| {
        let e = &x["error"];
        from_rpc(
            e["code"].as_i64().unwrap(),
            e["message"].as_str().unwrap(),
            "узел BSC (HTTP)",
            Op::Read,
            &e.to_string(),
        )
    };
    let range = v["getLogs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["ok"] == false)
        .unwrap();
    let e = err(range);
    assert_eq!(e.code, ErrorCode::BadResponse);
    assert!(e.message.contains("exceed maximum block range 5000"));
    // Принятый наибольший диапазон: toBlock − fromBlock = 5000.
    assert!(
        v["getLogs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["range_blocks"] == 5001 && x["ok"] == true)
    );
    let old = v["state"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["ok"] == false)
        .unwrap();
    assert_eq!(err(old).code, ErrorCode::StaleData);
    assert!(matches!(no_block(err(old).message), Cmp::NoBlock(t) if t.starts_with(NOT_CHECKED)));
}
