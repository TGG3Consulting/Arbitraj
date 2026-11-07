//! Файл: tests/core_start_sell_guard.rs
//! Что делает: решения 147 и 148 на настоящем ядре и SQLite во временной папке (площадок в реестре нет, моков нет).
//! 147: причина отказа исполнителя на входе — монета в ручном чёрном списке (запись справочника по записанным данным
//! CAKE: сеть BEP20 BingX, пул CAKE/USDT V2; отметка coin.blocked — настоящей настройкой), площадка исключена, монеты
//! нет в справочнике; путь очереди кандидатов — поле start строки «Кандидатов» и одна запись core.start_refused на
//! причину. 148: план — расчёт CAKE bingx → bsc_dex на 10 монет (записанные asks BingX и пул V2), факт продажи — средняя
//! цена того же пула на больший объём (математика адаптера на записанных резервах: хуже плана) или на тот же объём (не
//! хуже); N подряд хуже порога — монета в ручном чёрном списке с заметкой «Авто (решение 148)», журнал
//! core.sell_slip_block; продажа не хуже — счёт заново; серия, уже уведшая монету в список, начинается заново.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::venues::bsc_dex (математика пула),
//! tests/size_common.

mod size_common;

use std::path::PathBuf;

use arb::core::Core;
use arb::core::blacklist;
use arb::core::candidates;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::dry::Mode;
use arb::core::registry::{CoinEntry, Exclusion, Presence};
use arb::core::sell_slip::{self, Obs};
use arb::core::start_reason;
use arb::core::types::{CoinBlock, Side, TradeRequest, TradeResult, TradeStatus, VenueId};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use size_common::{CAKE, Rec, d};
use tokio::sync::broadcast;

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-guard-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе во временной папке. Возвращает ядро, настройки, журнал.
async fn start(name: &str) -> (Core, Store, Journal) {
    let d = dir(name);
    let db = Db::open(&d.join("t.sqlite")).unwrap();
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
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    (core, settings, journal)
}

/// Записи журнала этого вида. Ошибок нет.
async fn kinds(j: &Journal, kind: &str) -> Vec<String> {
    let all = j.list(500, None).await.unwrap();
    all.into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.message)
        .collect()
}

/// Запись справочника CAKE по записям: BingX (сеть BEP20 из config_getall) и DEX (пул CAKE/USDT V2).
fn entry(rec: &Rec) -> CoinEntry {
    let p = |v: &str, info| Presence {
        venue: VenueId(v.into()),
        ticker: "CAKE".into(),
        network: None,
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info,
        via: None,
    };
    CoinEntry {
        asset: size_common::calc::direction().asset,
        presence: vec![p("bingx", Some(rec.net.clone())), p("bsc_dex", None)],
        block: None,
        problem: None,
        wait: None,
        cex_cex: None,
    }
}

#[tokio::test]
async fn not_tradable_names_exact_reason() {
    let (_core, s, _j) = start("reason").await;
    let rec = Rec::load("guard-reason").await;
    let dir = size_common::calc::direction();
    let mut c = entry(&rec);
    assert!(c.directions().contains(&dir), "без блока направление есть");
    // Ручной чёрный список — настоящей настройкой, причина — тем же путём, что пересборка (blacklist::reason).
    let sc = Scope::Coin("CAKE".into());
    s.set("coin.blocked_note", sc.clone(), Some(json!("скам")), "тест")
        .await
        .unwrap();
    s.set("coin.blocked", sc, Some(json!(true)), "тест")
        .await
        .unwrap();
    // Решение 211: старая отметка по тикеру (не перенесённая миграцией) продолжает блокировать монету.
    let r = blacklist::reason(blacklist::manual(&s, &c.asset, &[]), &[], &Err("—".into()))
        .unwrap()
        .unwrap();
    c.block = Some(CoinBlock {
        asset: c.asset.clone(),
        reason: r,
        since: Utc::now(),
    });
    assert!(c.directions().is_empty());
    let why = start_reason::not_tradable(Some(&c), &dir);
    assert_eq!(why, "в чёрном списке: ручной чёрный список — скам");
    // Пара на бирже покупки не торгуется по статусу — исключение площадки с кодом.
    c.block = None;
    c.presence[0].excluded = vec![Exclusion::PairNotTrading(rec.pair.status + 1)];
    let why = start_reason::not_tradable(Some(&c), &dir);
    assert!(
        why.starts_with("на площадке покупки bingx монета исключена: пара не торгуется"),
        "{why}"
    );
    // Вывод на бирже закрыт.
    c.presence[0].excluded.clear();
    c.presence[0].can_send = false;
    assert_eq!(
        start_reason::not_tradable(Some(&c), &dir),
        "на площадке покупки bingx: закрыт вывод"
    );
    assert!(start_reason::not_tradable(None, &dir).contains("нет в рабочем справочнике"));
}

#[tokio::test]
async fn candidate_row_and_journal_show_refusal_once() {
    let (core, _s, j) = start("cand").await;
    let rec = Rec::load("guard-cand").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    let dir = calc.direction.clone();
    for _ in 0..2 {
        candidates::accepted(&core, &dir, calc.clone(), &Mode::Live).await;
    }
    assert!(core.deals().is_empty(), "сделки нет");
    let rows = core.candidates();
    let row = &rows.as_array().unwrap()[0];
    let start = row["start"].as_str().unwrap();
    assert!(
        start.starts_with("сделки нет — направление не торгуемо: монеты нет в рабочем справочнике"),
        "{start}"
    );
    // Два одинаковых отказа — одна запись (решение 147: на смену причины).
    let logged = kinds(&j, "core.start_refused").await;
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(logged[0].contains("CAKE bingx → bsc_dex: расчёт прошёл, сделки нет"));
}

/// Средняя цена продажи CAKE в записанный пул на qty монет (математика адаптера DEX). Ошибок нет.
fn pool_avg(rec: &Rec, qty: Decimal) -> Decimal {
    let out = swap(&rec.ctx, &rec.pool, CAKE, true, to_units(qty, 18).unwrap()).unwrap();
    to_decimal(out.amount_out, 18).unwrap() / qty
}

/// Закрытая продажей сделка: план — расчёт на 10 CAKE, факт — продажа 10 по цене avg в момент at.
fn deal(rec: &Rec, id: &str, avg: Decimal, at: chrono::DateTime<Utc>) -> Deal {
    let plan = size_common::calc::calc(rec, d("10"));
    let mut x = Deal::new(
        id.into(),
        plan.direction.clone(),
        serde_json::to_value(&plan).unwrap(),
        plan.profit_usd,
    );
    x.step = Step::Closed;
    x.sells.push(TradeResult {
        request: TradeRequest {
            venue: VenueId("bsc_dex".into()),
            asset: plan.direction.asset.clone(),
            side: Side::Sell,
            qty: d("10"),
            worst_price: avg,
            deal_id: id.into(),
            route: None,
            exit_position: false,
        },
        status: TradeStatus::Filled,
        filled_qty: d("10"),
        avg_price: avg,
        fee: Decimal::ZERO,
        fee_asset: "BNB".into(),
        venue_ref: id.into(),
        chain: None,
        finished_at: at,
        executor: None,
    });
    // Итог сделки: продажа хуже плана — минус (защита считает только минусовые сделки, Тигран 04.11).
    x.result_usd = Some((avg - plan.sell_avg) * d("10"));
    x
}

#[tokio::test]
async fn sell_slip_blocks_after_n_in_row_and_resets() {
    let (core, s, j) = start("slip").await;
    let rec = Rec::load("guard-slip").await;
    let plan = size_common::calc::calc(&rec, d("10")).sell_avg;
    let bad = pool_avg(&rec, d("20000"));
    let worse = sell_slip::slip_pct(plan, bad).unwrap();
    assert!(
        worse > Decimal::ZERO,
        "больший объём в тот же пул — цена хуже"
    );
    let max = worse / Decimal::TWO;
    // Чистые функции: хуже, хуже, не хуже (сброс), хуже, хуже — серия 2 из N = 2 только в конце.
    let o = |i: i64, p: Decimal| Obs {
        deal: format!("t{i}"),
        pct: p,
        at: Utc::now() + Duration::seconds(i),
        loss: true,
    };
    let obs = [o(1, worse), o(2, worse), o(3, Decimal::ZERO), o(4, worse)];
    // Серия из двух дошла до порога раньше последнего — заново: сейчас одна.
    assert_eq!(sell_slip::streak(&obs[..2], max, 2).len(), 2);
    assert_eq!(sell_slip::streak(&obs, max, 2).len(), 1);
    assert!(sell_slip::streak(&obs[..3], max, 2).is_empty(), "сброс");
    // Тигран 04.11: продажа хуже плана, но итог сделки не минусовой — серию сбрасывает.
    let plus = Obs {
        loss: false,
        ..o(5, worse)
    };
    assert!(
        sell_slip::streak(&[o(1, worse), plus], max, 1).is_empty(),
        "итог не минусовой"
    );
    // Защита не задана — выключена, одна запись в журнал.
    let t0 = Utc::now();
    let d1 = deal(&rec, "s1", bad, t0);
    let coin = d1.direction.asset.clone();
    arb::core::ledger::save(&core, &d1).await;
    sell_slip::after_close(&core, &d1).await;
    sell_slip::after_close(&core, &d1).await;
    assert_eq!(kinds(&j, "core.sell_slip_off").await.len(), 1);
    s.set(
        "guard.sell_slip_pct",
        Scope::Global,
        Some(json!(max.to_string())),
        "тест",
    )
    .await
    .unwrap();
    s.set(
        "guard.sell_slip_count",
        Scope::Global,
        Some(json!(3)),
        "тест",
    )
    .await
    .unwrap();
    // s1 хуже, s2 не хуже (тот же объём, что в плане) — сброс; s3, s4 хуже — пока два подряд.
    let good = pool_avg(&rec, d("10"));
    for (i, (id, avg)) in [("s2", good), ("s3", bad), ("s4", bad)].iter().enumerate() {
        let x = deal(&rec, id, *avg, t0 + Duration::seconds(i as i64 + 1));
        arb::core::ledger::save(&core, &x).await;
        sell_slip::after_close(&core, &x).await;
    }
    assert!(
        blacklist::manual(&s, &coin, &[]).is_none(),
        "после сброса только два подряд"
    );
    // Третья подряд — монета в ручном чёрном списке с заметкой, журнал Error.
    let x = deal(&rec, "s5", bad, t0 + Duration::seconds(10));
    arb::core::ledger::save(&core, &x).await;
    sell_slip::after_close(&core, &x).await;
    let note = blacklist::manual(&s, &coin, &[]).unwrap().unwrap();
    assert!(
        note.starts_with("Авто (решение 148): 3 продаж подряд хуже плана: −"),
        "{note}"
    );
    assert!(note.ends_with("(сделки s3, s4, s5)"), "{note}");
    assert_eq!(kinds(&j, "core.sell_slip_block").await.len(), 1);
    // Человек снял отметку — следующая плохая продажа монету снова не блокирует: серия начата заново.
    // Решение 211: автоблок записан по адресу монеты — снимает его человек там же.
    s.set(
        "coin.blocked",
        blacklist::coin_scope(&coin),
        Some(json!(false)),
        "тест",
    )
    .await
    .unwrap();
    let x = deal(&rec, "s6", bad, t0 + Duration::seconds(11));
    arb::core::ledger::save(&core, &x).await;
    sell_slip::after_close(&core, &x).await;
    assert!(blacklist::manual(&s, &coin, &[]).is_none());
    assert_eq!(kinds(&j, "core.sell_slip_block").await.len(), 1);
}
