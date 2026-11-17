//! Файл: tests/bsc_dex_quoter_refetch_382.rs
//! Что проверяет: решение 382 (задача t168) — расхождение расчёта по памяти с квотером перед сделкой ведёт к
//! перечитыванию пулов на голове, а не к отказу:
//! 1. какой отказ запускает перечитывание (exec/quoter_refetch.rs is_quoter_diff) — цифры живого бота 17.11, 09:19:34
//!    (покупка uniswap-bsc ESIM: расчёт 1301200000000000000000 против квотера 1213779234036790805839, допуск 0.11 %):
//!    отказ сверки против нас — да; «сравнить нельзя», цена хуже допустимой, MixedQuoter — нет;
//! 2. чем память пула отличается от узла (exec/quoter_refetch_note.rs) — на записанном пуле Uniswap V3 AAVE/USDT
//!    (tests/data/depth_2026-10-25, карта тиков из ответов TickLens): пропущенный Burn позиции (как событие, которое
//!    память не применила) виден как расхождение карты тиков ровно в двух тиках позиции, верх состояния совпадает;
//!    одинаковые карты — «совпадает»; окна без общих слов — «не сравнимы».
//!
//! Перечитывание на узле и повтор подготовки сделки (pinned, prepare, dry_build) тестом на записи не покрыты — записей
//! узла по этим пулам нет (список для серверного ИИ — docs/notes/t168-quoter-mismatch-2026-11-17.md).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{exec, quote, pools}.

mod depth_common;
mod native_common;

use std::str::FromStr;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::precheck;
use arb::venues::bsc_dex::exec::quoter_refetch::is_quoter_diff;
use arb::venues::bsc_dex::exec::quoter_refetch_note::{note, tick_diff};
use arb::venues::bsc_dex::quote::quoter::{self, Judged};

use depth_common::{aave_v3, rec};

/// Живой бот 17.11, 09:19:34: расчёт и квотер покупки ESIM (минимальные единицы монеты).
const LOCAL: &str = "1301200000000000000000";
const REMOTE: &str = "1213779234036790805839";

fn u(s: &str) -> U256 {
    U256::from_str(s).unwrap()
}

/// Окружение с допуском сверки 0.11 % (как на живом боте 17.11).
async fn ctx(tag: &str) -> arb::venues::bsc_dex::ctx::Ctx {
    let c = native_common::ctx(tag).await;
    c.settings
        .set(
            "dex.quoter_max_diff_pct",
            Scope::Venue("bsc_dex".into()),
            Some(json!("0.11")),
            "тест",
        )
        .await
        .unwrap();
    c
}

#[tokio::test]
async fn live_esim_mismatch_triggers_refetch() {
    let c = ctx("t382-esim").await;
    let (e, _, _) = aave_v3(&rec());
    let j = quoter::judge(&c, u(LOCAL), u(REMOTE), Decimal::ZERO).unwrap();
    let Judged::Diff { pct, .. } = &j else {
        panic!("ждали расхождение: {j:?}");
    };
    // Живая строка журнала: «расхождение 7.2023613118231504852837094600%».
    assert!(pct.to_string().starts_with("7.20236131182315"), "{pct}");
    assert!(!j.favours_us());
    let err = precheck::verdict(&c, &e, j, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
    assert!(err.message.contains(LOCAL) && err.message.contains(REMOTE));
    assert!(is_quoter_diff(&err));
    // «Сравнить нельзя» (влияние крючка не считается) — не память: перечитывания нет.
    let bad = quoter::judge(&c, u(LOCAL), u(REMOTE), Decimal::from(101)).unwrap();
    assert!(matches!(bad, Judged::Bad(_)));
    let err = precheck::verdict(&c, &e, bad, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
    assert!(!is_quoter_diff(&err));
    // Цена хуже допустимой и MixedQuoter — тоже QuoteMismatch, но не сверка квотера пула.
    let price = c.err(ErrorCode::QuoteMismatch, "цена хуже допустимой");
    assert!(!is_quoter_diff(&price));
    let mixed = c.err(
        ErrorCode::QuoteMismatch,
        format!("x Y: расчёт {LOCAL} против MixedQuoter {REMOTE} — расхождение 7% > 0.11% (dex.quoter_max_diff_pct)"),
    );
    assert!(!is_quoter_diff(&mixed));
}

#[test]
fn missed_burn_shows_in_tick_map() {
    let (mem, _, _) = aave_v3(&rec());
    let map = mem.ticks.clone().unwrap();
    // Позиция между первым и последним тиком записи; Δ — не больше gross обоих (Burn не уводит gross ниже нуля).
    let (lo, a) = map.ticks.iter().next().map(|(t, d)| (*t, *d)).unwrap();
    let (hi, b) = map.ticks.iter().next_back().map(|(t, d)| (*t, *d)).unwrap();
    assert!(lo < hi);
    let delta = a.gross.min(b.gross) as i128;
    // Узел: Burn применён. Память: нет (событие не применено) — верх тот же.
    let mut fresh = mem.clone();
    assert!(fresh.ticks.as_mut().unwrap().modify(lo, hi, -delta));
    let d = tick_diff(mem.ticks.as_ref().unwrap(), fresh.ticks.as_ref().unwrap());
    assert_eq!(d.words, Some(map.words));
    assert_eq!(d.ticks, vec![lo, hi]);
    let text = note(&mem, &fresh, 1);
    assert!(text.contains("верх состояния совпадает"), "{text}");
    assert!(text.contains("НЕ совпадает: 2 тик"), "{text}");
    // Одинаковые карты — совпадает.
    let same = note(&mem, &mem.clone(), 1);
    assert!(
        same.contains("совпадает с узлом") && same.contains("совпадает"),
        "{same}"
    );
    assert!(!same.contains("НЕ совпадает"), "{same}");
    // Окна без общих слов — карты не сравнимы.
    let mut far = map.clone();
    let width = map.words.1 - map.words.0 + 1;
    far.words = (map.words.0 + width, map.words.1 + width);
    let d = tick_diff(&map, &far);
    assert_eq!(d.words, None);
    assert!(d.ticks.is_empty());
}
