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
//! 5. ревью 3 (чистая логика): вид «каждым блоком» — сравнение только на блоке памяти (per_block_gate); узкое окно свежих
//!    логов при отказе узла по диапазону (narrow_from, by_logs: свежие — отставание, нет — сравнение на H); пары Multi
//!    доказывают пропуск (own); пропуск, уже применённый текущей памятью, не метится (still_missing); охранник учёта
//!    закрывает запись при Drop, keep_only чистит.
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
use arb::venues::bsc_dex::check::memory_proof::{Proof, by_logs};
use arb::venues::bsc_dex::check::memory_proof_rules::{
    Unapplied, filter, narrow_from, own, per_block_gate, positions, unapplied,
};
use arb::venues::bsc_dex::exec::precheck;
use arb::venues::bsc_dex::exec::quoter_refetch::is_quoter_diff;
use arb::venues::bsc_dex::exec::quoter_refetch_note::{differs, note, tick_diff};
use arb::venues::bsc_dex::exec::quoter_refetch_proof::{mark, still_missing};
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
    // Ревью 2 Н2, ревью 3 Н3: одно доказательство на (пул, позиция); охранник закрывает запись и без явного итога
    // (паника / отмена задачи — Drop); доказанная ошибка той же позиции не повторяется; новая позиция — можно;
    // записи пулов, которых больше нет, чистятся.
    let reg = Registry::default();
    let (e, _, _) = aave_v3(&rec());
    let r = e.pool.reference.clone();
    let m = reg.proofs();
    let g = m.begin(&r, (100, Some(7))).expect("первое");
    assert!(m.begin(&r, (100, Some(7))).is_none(), "уже идёт");
    drop(g);
    let mut g = m
        .begin(&r, (100, Some(7)))
        .expect("Drop закрыл — можно снова");
    g.proven(true);
    drop(g);
    assert!(
        m.begin(&r, (100, Some(7))).is_none(),
        "ошибка доказана — не повторять"
    );
    let g = m.begin(&r, (130, None)).expect("новая позиция");
    drop(g);
    let mut g = m.begin(&r, (140, None)).expect("ещё позиция");
    g.proven(true);
    drop(g);
    m.keep_only(|_| false);
    assert!(
        m.begin(&r, (140, None)).is_some(),
        "пула больше нет — запись вычищена"
    );
}

#[test]
fn review3_rules() {
    // Ревью 3 (чистая логика, позиции логов — числа).
    let (mut mem, _, _) = aave_v3(&rec());
    mem.synced_block = 100;
    mem.synced_log = Some(7);
    // Б1: вид «каждым блоком» (LB V2.1): память на H − 1 — сравнивать нельзя (отставание); прочитан целиком на H — можно.
    let mut lb = native_common::entry("lb_v21", None);
    lb.synced_block = 119;
    lb.synced_log = None;
    assert!(per_block_gate(&lb, 120).is_some());
    lb.synced_block = 120;
    assert!(per_block_gate(&lb, 120).is_none());
    // Событийный вид — гейта нет.
    assert!(per_block_gate(&mem, 120).is_none());
    // Б2: узкое окно свежих логов — последние dex.quoter_memory_lag_blocks блоков, не раньше блока памяти.
    assert_eq!(narrow_from(100, 120, 3), 117);
    assert_eq!(narrow_from(119, 120, 3), 119);
    assert_eq!(narrow_from(0, 2, 3), 0);
    // Свежий неприменённый лог окна — отставание, не ошибка.
    assert!(matches!(
        by_logs(&mem, Unapplied::Fresh(1, (118, 1)), 120, 3),
        Some(Proof::Lag(_))
    ));
    // Неприменённых нет — решения по логам нет (дальше сравнение с узлом на H).
    assert_eq!(by_logs(&mem, Unapplied::None, 120, 3), None);
    // Пропуск у своего контракта — ошибка с позицией пропуска (для Н1).
    assert!(matches!(
        by_logs(&mem, Unapplied::Missed((105, 2)), 120, 3),
        Some(Proof::Wrong {
            strong: true,
            missed: Some((105, 2)),
            ..
        })
    ));
    // Н2: пары многомонетного пула (Multi) — логи контракта доказывают пропуск; вид «каждым блоком» — нет.
    let mut multi = mem.clone();
    multi.pool.reference = PoolRef::Multi {
        contract: alloy_primitives::Address::repeat_byte(0x33),
        token0: alloy_primitives::Address::repeat_byte(0x01),
        token1: alloy_primitives::Address::repeat_byte(0x02),
    };
    assert!(own(&multi));
    assert!(own(&mem));
    assert!(!own(&lb));
    // Н1: пропуск, который текущая память уже применила (лог опоздал больше допуска), — не метить.
    let mut now = mem.clone();
    now.synced_block = 106;
    now.synced_log = Some(0);
    assert!(!still_missing(Some(&now), Some((105, 2))));
    assert!(still_missing(Some(&mem), Some((105, 2))));
    assert!(still_missing(Some(&mem), None));
    assert!(!still_missing(None, Some((105, 2))));
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
