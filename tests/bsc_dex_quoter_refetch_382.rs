//! Файл: tests/bsc_dex_quoter_refetch_382.rs
//! Что проверяет: решение 382 (задача t168) — расхождение расчёта по памяти с квотером перед сделкой ведёт к
//! перечитыванию пулов на голове, а не к отказу:
//! 1. какой отказ запускает перечитывание (exec/quoter_refetch.rs is_quoter_diff) — цифры живого бота 17.11, 09:19:34
//!    (покупка uniswap-bsc ESIM: расчёт 1301200000000000000000 против квотера 1213779234036790805839, допуск 0.11 %):
//!    отказ сверки против нас — да (и MixedQuoter с цифрами); «сравнить нельзя», цена хуже допустимой, «расхождение не
//!    считается» — нет;
//! 2. чем память пула отличается от узла (exec/quoter_refetch_note.rs) — на записанном пуле Uniswap V3 AAVE/USDT
//!    (tests/data/depth_2026-10-25, карта тиков из ответов TickLens): пропущенный Burn позиции (как событие, которое
//!    память не применила) виден как расхождение карты тиков ровно в двух тиках позиции, верх состояния совпадает;
//!    одинаковые карты — «совпадает»; окна без общих слов — «не сравнимы».
//!
//! 3. отметка «нужно полное чтение» (pools/settle_reread.rs, Need::Reread) на записанном пуле: недостоверен, ожидание
//!    логов её не заменяет и не снимает, любой лог — на чтение, снимает только достоверная запись;
//! 4. ревью 2, Б1 — отставание не ошибка (чистая логика, позиции логов — числа): пропущенное событие после блока памяти
//!    у затихшего пула — ошибка; тихий пул — сравнение на H; свежие логи — отставание; фильтр V4 / Infinity — менеджер и
//!    topic1 = PoolId (check/memory_proof.rs unapplied, positions, filter); отметка (Н1) и учёт доказательств (Н2).
//!    Сам eth_getLogs и чтение узла (prove) — без записи.
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

use alloy::rpc::types::Log;
use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::memory_proof::{Unapplied, filter, positions, unapplied};
use arb::venues::bsc_dex::exec::precheck;
use arb::venues::bsc_dex::exec::quoter_refetch::is_quoter_diff;
use arb::venues::bsc_dex::exec::quoter_refetch_note::{differs, note, tick_diff};
use arb::venues::bsc_dex::exec::quoter_refetch_proof::mark;
use arb::venues::bsc_dex::pools::registry::Registry;
use arb::venues::bsc_dex::pools::settle::Need;
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
    // Цена хуже допустимой — тоже QuoteMismatch, но не сверка с квотером.
    let price = c.err(ErrorCode::QuoteMismatch, "цена хуже допустимой");
    assert!(!is_quoter_diff(&price));
    // MixedQuoter с цифрами (умный маршрут) — тоже перечитывание; «расхождение не считается» — нет.
    let mixed = c.err(
        ErrorCode::QuoteMismatch,
        format!(
            "x Y: расчёт {LOCAL} против MixedQuoter {REMOTE} — расхождение 7% > 0.11% (dex.quoter_max_diff_pct)"
        ),
    );
    assert!(is_quoter_diff(&mixed));
    let blind = c.err(
        ErrorCode::QuoteMismatch,
        format!(
            "x Y: расчёт {LOCAL} против MixedQuoter {REMOTE} — расхождение не считается > 0.11% (dex.quoter_max_diff_pct)"
        ),
    );
    assert!(!is_quoter_diff(&blind));
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

#[test]
fn reread_mark_waits_for_full_read() {
    let (e, _, _) = aave_v3(&rec());
    let r = e.pool.reference.clone();
    let reg = Registry::default();
    reg.put(e.clone());
    let h = reg.holds();
    // Память доказанно не равна узлу — пул недостоверен и ждёт полного чтения.
    assert!(h.reread(&reg, &r, 100));
    assert!(!reg.get(&r).unwrap().valid);
    assert!(h.rereading(&r));
    // Ожидание логов (решение 357) отметку не заменяет, снятие ожидания достоверность не возвращает.
    h.hold(&reg, &r, (101, Need::Follow), Vec::new(), false);
    assert!(h.rereading(&r));
    h.release(&reg, &r);
    assert!(h.rereading(&r));
    assert!(!reg.get(&r).unwrap().valid);
    // Любой лог пула — сразу на полное чтение.
    assert!(h.lost(&r, 100, None));
    // Недостоверный пул повторно не отмечается.
    assert!(!h.reread(&reg, &r, 102));
    // Полное чтение (достоверная запись) снимает отметку.
    reg.put(e.clone());
    assert!(!h.rereading(&r));
    assert!(reg.get(&r).unwrap().valid);
}

#[test]
fn lag_is_not_error() {
    // Ревью 2, Б1 (чистая логика; позиции логов — числа, не записанный ответ узла): память пула — блок 100, logIndex 7;
    // голова H = 120, допуск отставания dex.quoter_memory_lag_blocks = 3.
    let (mut mem, _, _) = aave_v3(&rec());
    mem.synced_block = 100;
    mem.synced_log = Some(7);
    let applied = |b: u64, i: u64| mem.has_applied(b, i);
    // 1. Событие пропущено ПОСЛЕ блока памяти, пул затих: лог блока 105 у узла есть, память его не применила — ошибка.
    assert_eq!(
        unapplied(&[(100, 5), (105, 2)], applied, 120, 3),
        Unapplied::Missed((105, 2))
    );
    // 2. Пул тихий: у узла нет логов после применённых — неприменённых нет, сравнение с узлом на H (п. 4, differs).
    assert_eq!(
        unapplied(&[(100, 5), (100, 7)], applied, 120, 3),
        Unapplied::None
    );
    // Отставание: неприменённый лог только свежий (блок 118 > 120 − 3) — не ошибка.
    assert_eq!(
        unapplied(&[(118, 1)], applied, 120, 3),
        Unapplied::Fresh(1, (118, 1))
    );
    // Лог того же блока после logIndex памяти и старше допуска — тоже пропуск.
    assert_eq!(
        unapplied(&[(100, 9)], applied, 120, 3),
        Unapplied::Missed((100, 9))
    );
    // Отменённые реоргом логи и логи без позиции не учитываются.
    let log = |b: u64, i: u64, removed: bool| Log {
        block_number: Some(b),
        log_index: Some(i),
        removed,
        ..Default::default()
    };
    assert_eq!(
        positions(&[log(105, 2, true), log(106, 1, false), Log::default()]),
        vec![(106, 1)]
    );
    // 3. Пул V4 / Infinity (Singleton): фильтр — менеджер и topic1 = PoolId, диапазон [S, H] — как у подписки пачки.
    let id = alloy_primitives::B256::repeat_byte(0x11);
    let manager = alloy_primitives::Address::repeat_byte(0x22);
    let mut v4 = mem.clone();
    v4.pool.reference = PoolRef::Singleton { manager, id };
    let f = filter(&v4, 100, 120);
    assert_eq!(f["address"], json!(manager));
    assert_eq!(f["topics"][1], json!([id]));
    assert_eq!(f["fromBlock"], json!("0x64"));
    assert_eq!(f["toBlock"], json!("0x78"));
    // Свой контракт — по адресу пула.
    let f = filter(&mem, 100, 120);
    assert_eq!(f["address"], json!([mem.pool.reference.address()]));
    // Отметка по доказанной ошибке (Н1): память с тех пор перечитана целиком — нет; сильная (пропуск, карта тиков) —
    // да, даже если позиция ушла событием; только верх — если позиция та же.
    assert!(!mark(true, (100, Some(7)), Some((130, None))));
    assert!(mark(true, (100, Some(7)), Some((101, Some(2)))));
    assert!(mark(false, (100, Some(7)), Some((100, Some(7)))));
    assert!(!mark(false, (100, Some(7)), Some((101, Some(2)))));
    assert!(!mark(true, (100, Some(7)), None));
}

#[test]
fn one_proof_per_position() {
    // Ревью 2, Н2: одно доказательство на (пул, позиция); доказанная ошибка той же позиции не повторяется; иной итог —
    // можно снова; новая позиция — можно.
    let reg = Registry::default();
    let (e, _, _) = aave_v3(&rec());
    let r = e.pool.reference.clone();
    let m = reg.proofs();
    assert!(m.begin(&r, (100, Some(7))));
    assert!(!m.begin(&r, (100, Some(7))), "уже идёт");
    m.finish(&r, (100, Some(7)), false);
    assert!(m.begin(&r, (100, Some(7))), "итог не ошибка — можно снова");
    m.finish(&r, (100, Some(7)), true);
    assert!(
        !m.begin(&r, (100, Some(7))),
        "ошибка доказана — не повторять"
    );
    assert!(m.begin(&r, (130, None)), "новая позиция");
}

#[test]
fn differs_by_top_or_ticks() {
    let (mem, _, _) = aave_v3(&rec());
    assert!(!differs(&mem, &mem.clone()));
    let mut fresh = mem.clone();
    let map = fresh.ticks.as_mut().unwrap();
    let (lo, a) = map.ticks.iter().next().map(|(t, d)| (*t, *d)).unwrap();
    let (hi, b) = map.ticks.iter().next_back().map(|(t, d)| (*t, *d)).unwrap();
    assert!(map.modify(lo, hi, -(a.gross.min(b.gross) as i128)));
    assert!(differs(&mem, &fresh));
}
