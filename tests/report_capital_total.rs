//! Файл: tests/report_capital_total.rs
//! Что проверяет: решение 256 (дополнение — аудит отчёта «Сделки, лимиты и капитал») на настоящем ядре (SQLite во
//! временной папке, tests/report_common) и записанных данных (400 сделок tests/data/report_2026-11-05/deals.csv, пул
//! CAKE/USDT V2 из tests/size_common):
//! 1) итог периода книги = итог «Итогов сделок» за то же окно: те же сделки (report/rows.rs selected) и та же сумма
//!    «факт + оценка»; с LibreOffice — строка «Итого» таблицы 2 = «Итог факт + оценка» листа «Итог» «Итогов сделок»;
//! 2) крайние случаи: остановленная с монетами и «Беру на себя» → «Вернуть боту» (покупки нет — «Нет», занятость у
//!    исходной до конца сделки «<id>r»), вложение 0 с газом (в итоге, покупки нет), возврат USDT «вернулись» (занято
//!    до записи «USDT вернулись»), возврат «идёт» (занято до выгрузки), закрытая без записи конца (вне расчёта, пометка),
//!    открытая, закрытая вчера с идущим возвратом (в листе — для занятости, не в итоге), цикл DEX ↔ DEX (пулы по плану);
//! 3) адреса: пулы маршрута ордера DEX, хеши свопов, id ордеров биржи, вывод и зачисление перевода, кошелёк DEX (адрес
//!    не читается — DEX не подключена: причина, а не выдумка);
//! 4) лимиты: значения листа = чтение бота (core/limits_parts.rs parts, core/venue_dir.rs thresholds), «откуда взято».
//!
//! Книги — в REPORT_CAPITAL_TOTAL_OUT и REPORT_CAPITAL_RESULTS_OUT (если заданы) для пересчёта и сверки в Python.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::report, tests/report_common, tests/size_common, tests/cap_common. Моков нет.

mod cap_common;
mod report_common;
mod size_common;

use std::collections::BTreeSet;

use alloy_primitives::B256;
use arb::core::deal::{Deal, Step};
use arb::core::types::{
    ChainTx, CostKind, DexRoute, PoolRef, RouteHop, RouteLeg, RouteMode, SendMethod, Side,
    Transfer, TransferStatus, TxOutcome, VenueId,
};
use arb::core::usdt_return_state::UsdtReturn;
use arb::gateway::contract::VenueKind;
use arb::report::rows_time::{Window, calendar_from};
use arb::settings::value::Scope;
use cap_common::{bought, near, num, recalc, trade};
use chrono::{Duration, Utc};
use report_common::{d, deal, dir, recorded, start};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::json;

/// Транзакция DEX в блоке (хеш — по байту). Ошибок нет.
fn chain(b: u8) -> ChainTx {
    ChainTx {
        hash: B256::repeat_byte(b),
        nonce: 1,
        channels: vec![],
        method: SendMethod::Single,
        outcome: TxOutcome::Included,
        block: Some(1),
        gas_used: None,
        gas_bnb: None,
        tip_bnb: None,
        cancel_hash: None,
        cancel_gas_bnb: None,
        approve_gas_bnb: None,
        tip_hash: None,
        tip_gas_bnb: None,
    }
}

/// Перевод монеты сделки. Ошибок нет.
fn transfer(x: &Deal, venue: &str, id: &str, hash: &str) -> Transfer {
    Transfer {
        venue: VenueId(venue.into()),
        asset: x.direction.asset.clone(),
        network: "BSC".into(),
        amount: Decimal::ONE,
        status: TransferStatus::Credited,
        venue_ref: id.into(),
        tx_hash: Some(hash.into()),
        fee: None,
        updated_at: x.started_at,
    }
}

#[tokio::test]
async fn capital_total_equals_results_and_edges() {
    let rec = size_common::Rec::load("cap-total").await;
    let pool = rec.pool.pool.reference.clone();
    let route = DexRoute {
        mode: RouteMode::Direct,
        legs: vec![RouteLeg {
            share: Decimal::ONE,
            hops: vec![RouteHop {
                family: rec.pool.family.id.to_string(),
                kind: rec.pool.pool.kind,
                pool: pool.clone(),
            }],
            expected_out: Decimal::ONE,
        }],
        block: rec.pool.pool.block,
        gas_units: 0,
        executable: true,
        exec_note: None,
    };
    let p = dir("cap-total");
    let (core, st) = start(&p).await;
    let kinds = vec![
        (VenueId("bingx".into()), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
        (VenueId("mexc".into()), VenueKind::Cex),
    ];
    *core.inner.kinds.write().unwrap() = kinds.clone();
    st.venue_kinds()
        .set(kinds.iter().map(|(v, k)| (v.0.clone(), *k)));
    let set = |k: &'static str, s: Scope, v: serde_json::Value| {
        let st = st.clone();
        async move { st.set(k, s, Some(v), "тест").await.unwrap() }
    };
    set("report.snapshot_timeout_ms", Scope::Global, json!(3000)).await;
    set("uncapped.keep_days", Scope::Global, json!(14)).await;
    set("money.max_per_trade_usd", Scope::Global, json!("50")).await;
    set(
        "money.max_per_trade_usd",
        Scope::VenueDir("mexc:cex_dex".into()),
        json!("30"),
    )
    .await;
    set(
        "money.max_per_trade_usd",
        Scope::Venue("bsc_dex".into()),
        json!("40"),
    )
    .await;
    set("money.min_profit_usd", Scope::Global, json!("0.1")).await;
    set(
        "money.min_profit_usd",
        Scope::VenueDir("bingx:dex_cex".into()),
        json!("0.3"),
    )
    .await;
    set("money.min_profit_pct", Scope::Global, json!("0.5")).await;
    set("money.max_in_transit_usd", Scope::Global, json!("300")).await;
    set("money.max_per_coin_usd", Scope::Global, json!("200")).await;
    set(
        "risk.max_daily_loss_usd",
        Scope::Venue("mexc".into()),
        json!("15"),
    )
    .await;
    set("risk.max_daily_loss_usd", Scope::Global, json!("20")).await;
    st.accounts()
        .add("w1", "Основной", "s1", "q1", "тест")
        .await
        .unwrap();

    // 400 записанных сделок с покупкой — закрыты тем же вызовом, что у бота.
    for mut x in recorded("r") {
        bought(&mut x);
        arb::core::ledger::finish(&core, &mut x, Step::Closed, "запись 05.11").await;
    }
    let mut h = deal(0, "h");
    bought(&mut h);
    (h.proceeds, h.qty) = (Decimal::ZERO, d("1"));
    arb::core::ledger::finish(&core, &mut h, Step::Halted, "тест: остановлена").await;
    let mut t = deal(1, "t");
    bought(&mut t);
    (t.proceeds, t.qty) = (Decimal::ZERO, d("1"));
    arb::core::ledger::finish(&core, &mut t, Step::TakenOver, "тест: Беру на себя").await;
    // «Вернуть боту» (core/manual_return.rs): новая сделка «<id>r» без покупки, вложено — доля исходной, продаётся.
    let mut tr = Deal::new(
        format!("{}r", t.id),
        t.direction.clone(),
        t.plan.clone(),
        t.plan_profit_usd,
    );
    (tr.invested, tr.qty, tr.step, tr.started_at) = (
        t.invested,
        d("1"),
        Step::Selling,
        t.started_at + Duration::hours(1),
    );
    arb::core::ledger::save(&core, &tr).await;
    let mut z = deal(2, "z");
    (z.invested, z.proceeds) = (Decimal::ZERO, Decimal::ZERO);
    arb::core::deal_book::spend(&mut z, CostKind::Gas, d("0.0021"));
    arb::core::ledger::finish(&core, &mut z, Step::Closed, "тест: покупка не прошла").await;
    // DEX → MEXC: своп на DEX кошельком w1, вывод, зачисление, продажа на MEXC, возврат USDT «вернулись».
    let mut u = deal(3, "u");
    u.direction.buy_on = VenueId("bsc_dex".into());
    u.direction.sell_on = VenueId("mexc".into());
    u.dex_account = Some(arb::core::types::DexAccountId("w1".into()));
    let at = u.started_at;
    u.buy = Some(trade(
        &u,
        "bsc_dex",
        Side::Buy,
        d("1"),
        u.invested,
        "0x07",
        Some(chain(7)),
        Some(route.clone()),
        at,
    ));
    u.transfers = vec![
        transfer(&u, "bsc_dex", "w-1", "0xaa"),
        transfer(&u, "mexc", "dep-1", "0xaa"),
    ];
    u.sells = vec![trade(
        &u,
        "mexc",
        Side::Sell,
        d("1"),
        u.proceeds,
        "mx-77",
        None,
        None,
        at,
    )];
    u.at = VenueId("mexc".into());
    let amount = d("27.655454087068799999999999694");
    let (mx, dx) = (VenueId("mexc".into()), VenueId("bsc_dex".into()));
    u.usdt_return = Some(UsdtReturn::Pending {
        amount,
        from: mx.clone(),
        to: dx.clone(),
    });
    arb::core::ledger::finish(&core, &mut u, Step::Closed, "тест: продано").await;
    let done = UsdtReturn::Done {
        amount,
        credited: d("27.59"),
        from: mx.clone(),
        to: dx.clone(),
        sent: Some(d("27.6")),
        fee_usd: None,
    };
    let note = format!("USDT вернулись: {amount} mexc → bsc_dex, зачислено 27.59");
    arb::core::usdt_return_state::set(&core, &u.id, done, &note).await;
    // Возврат «идёт» — занято до выгрузки.
    let mut q = deal(9, "q");
    bought(&mut q);
    q.usdt_return = Some(UsdtReturn::Pending {
        amount,
        from: mx.clone(),
        to: dx.clone(),
    });
    arb::core::ledger::finish(&core, &mut q, Step::Closed, "тест: возврат идёт").await;
    // Закрыта без записи конца (запись учёта без конечной записи хронологии).
    let mut ne = deal(4, "ne");
    bought(&mut ne);
    (ne.step, ne.result_usd) = (Step::Closed, Some(ne.proceeds - ne.invested));
    // Конец неизвестен — день по старту (как «Итоги сделок»): старт сегодня, чтобы сделка была в окне.
    ne.started_at = Utc::now() - Duration::minutes(5);
    arb::core::ledger::save(&core, &ne).await;
    // Открытая на продаже.
    let mut op = deal(6, "op");
    bought(&mut op);
    (op.step, op.qty) = (Step::Selling, d("1"));
    arb::core::ledger::save(&core, &op).await;
    // Закрыта вчера, возврат USDT идёт — касается окна «текущий день» занятостью, в итог не входит.
    let mut y = deal(7, "y");
    bought(&mut y);
    y.step = Step::Closed;
    y.result_usd = Some(y.proceeds - y.invested);
    y.usdt_return = Some(UsdtReturn::Pending {
        amount,
        from: mx.clone(),
        to: dx.clone(),
    });
    let end = Utc::now() - Duration::days(1);
    y.timeline.push((
        end,
        format!("{}тест", arb::core::deal_book::end_prefix(Step::Closed)),
    ));
    arb::core::ledger::save(&core, &y).await;
    // Цикл DEX ↔ DEX: своп транзакцией, маршрут — в плане цикла (CyclePlan.cycle.hops).
    let mut c = deal(8, "c");
    c.direction.buy_on = VenueId("bsc_dex".into());
    c.direction.sell_on = VenueId("bsc_dex".into());
    c.at = VenueId("bsc_dex".into());
    c.dex_account = Some(arb::core::types::DexAccountId("w1".into()));
    c.plan = json!({"cycle": {"hops": [{"family": rec.pool.family.id, "pool": pool}]}});
    c.buy = Some(trade(
        &c,
        "bsc_dex",
        Side::Sell,
        d("1"),
        c.invested,
        "0x09",
        Some(chain(9)),
        None,
        c.started_at,
    ));
    arb::core::ledger::finish(&core, &mut c, Step::Closed, "цикл DEX ↔ DEX исполнен").await;

    let deals = core.deals();
    let now = Utc::now();
    let w = Window {
        from: Some(calendar_from(1, now)),
        to: None,
        now,
    };
    let missed = core.uncapped_report(w.from, w.to).await.unwrap();
    let inputs = arb::report::cap_send::gather(&core, &deals).await;
    assert_eq!(inputs.rows.dex, BTreeSet::from(["bsc_dex".to_string()]));
    assert_eq!(inputs.keep_days, Ok(14));
    let data = arb::report::cap_send::capital_data(&deals, &missed, &w, "текущий день", &inputs);

    // 1) Итог периода = «Итоги сделок» за то же окно.
    let res = arb::report::rows::rows(&deals, &w, &inputs.rows.dex);
    let mut a: Vec<&str> = data
        .deals
        .iter()
        .filter(|r| r.in_total)
        .map(|r| r.id.as_str())
        .collect();
    let mut b: Vec<&str> = res.iter().map(|r| r.id.as_str()).collect();
    a.sort();
    b.sort();
    assert_eq!(a, b, "те же сделки");
    let total = |f: Option<f64>, e: Option<f64>| f.unwrap_or(0.0) + e.unwrap_or(0.0);
    let mine: f64 = data
        .deals
        .iter()
        .filter(|r| r.in_total)
        .map(|r| total(r.fact, r.estimate))
        .sum();
    let theirs: f64 = res.iter().map(|r| total(r.fact, r.estimate)).sum();
    near(mine, theirs, "итог периода = «Итоги сделок»");

    // 2) Крайние случаи.
    let row = |id: &str| {
        (data.deals.iter().find(|r| r.id == id)).unwrap_or_else(|| {
            panic!(
                "нет строки {id}: {:?}",
                deals
                    .iter()
                    .find(|x| x.id == id)
                    .map(|x| (x.step, x.started_at, &x.timeline))
            )
        })
    };
    let (rh, rt, rr) = (row(&h.id), row(&t.id), row(&tr.id));
    assert!(rh.hold.counted && rh.hold.until.is_none() && rh.estimate.is_some() && rh.in_total);
    assert!(rt.hold.counted && rt.hold.until.is_none() && rt.hold.note.contains(&tr.id));
    assert!(!rr.bought && !rr.hold.counted && rr.hold.note.contains(&t.id) && !rr.in_total);
    let rz = row(&z.id);
    assert!(
        !rz.bought && rz.in_total && (rz.spent - 0.0021).abs() < 1e-12 && rz.fact == Some(-0.0021)
    );
    let ru = row(&u.id);
    let back = deals
        .iter()
        .find(|x| x.id == u.id)
        .unwrap()
        .timeline
        .iter()
        .rev()
        .find(|(_, s)| s.starts_with("USDT вернулись"))
        .unwrap()
        .0;
    assert_eq!(ru.hold.until, Some(back));
    assert!(ru.hold.place.contains("DEX, кошелёк «Основной»"));
    assert!(row(&q.id).hold.until.is_none() && row(&q.id).hold.counted);
    let rn = row(&ne.id);
    assert!(
        rn.end.is_none() && rn.end_note.contains("не найден") && !rn.hold.counted && rn.in_total
    );
    assert!(row(&op.id).why_not.contains("открыта") && row(&op.id).hold.counted);
    let ry = row(&y.id);
    assert!(!ry.in_total && ry.why_not.contains("до начала окна") && ry.hold.until.is_none());

    // 3) Адреса из учёта.
    let addr = format!("{} {:#x}", rec.pool.family.id, pool.address());
    assert_eq!(ru.addr.buy_pools, addr);
    assert_eq!(ru.addr.buy_tx, format!("{:#x}", B256::repeat_byte(7)));
    assert_eq!(
        (ru.addr.buy_order.as_str(), ru.addr.sell_orders.as_str()),
        ("", "mx-77")
    );
    assert_eq!(ru.addr.withdraws, "DEX: id w-1, хеш 0xaa");
    assert_eq!(ru.addr.deposits, "MEXC: id dep-1, хеш 0xaa");
    assert!(
        ru.addr.wallet.contains("адрес не прочитан"),
        "{}",
        ru.addr.wallet
    );
    assert_eq!(row(&c.id).addr.buy_pools, format!("по плану цикла: {addr}"));
    assert!(row(&c.id).hold.note.starts_with("цикл DEX ↔ DEX"));
    assert_eq!(rh.addr.buy_order, format!("{}b", h.id));
    assert!(matches!(pool, PoolRef::Contract(_)));

    // 4) Лимиты = чтение бота.
    for (a, ka) in &kinds {
        for (bb, _) in &kinds {
            if a == bb && *ka != VenueKind::Dex {
                continue;
            }
            let dd = arb::core::types::Direction {
                asset: arb::core::types::Asset {
                    ticker: "CAKE".into(),
                    bsc_contract: Some(size_common::CAKE),
                },
                buy_on: a.clone(),
                sell_on: bb.clone(),
            };
            let bot =
                arb::core::limits_parts::parts(&core, &dd, &arb::core::dry::Mode::Live).unwrap();
            let what = arb::notify::human::route(&a.0, &bb.0);
            let lr = data
                .limits
                .iter()
                .find(|r| r.setting == "money.max_per_trade_usd" && r.what.starts_with(&what))
                .unwrap();
            near(
                lr.value.clone().unwrap(),
                bot.per_trade.to_f64().unwrap(),
                "лимит на сделку",
            );
            let vd = arb::core::venue_dir::of(&core, &dd);
            let (usd, _) = arb::core::venue_dir::thresholds(&st, vd).unwrap();
            let lp = data
                .limits
                .iter()
                .find(|r| r.setting == "money.min_profit_usd" && r.what.starts_with(&what))
                .unwrap();
            near(
                lp.value.clone().unwrap(),
                usd.to_f64().unwrap(),
                "порог прибыли",
            );
        }
    }
    let src = |s: &str, w: &str| {
        data.limits
            .iter()
            .find(|r| r.setting == s && r.what.starts_with(w))
            .unwrap()
            .source
            .clone()
    };
    assert_eq!(
        src("money.max_per_trade_usd", "MEXC → DEX"),
        "пара «MEXC · биржа → DEX»"
    );
    assert_eq!(src("money.max_per_trade_usd", "DEX → MEXC"), "площадка DEX");
    assert_eq!(
        src("money.max_per_trade_usd", "BingX → MEXC"),
        "вся система"
    );
    assert_eq!(src("risk.max_daily_loss_usd", "MEXC"), "площадка MEXC");
    assert_eq!(src("risk.max_daily_loss_usd", "BingX"), "вся система");
    let tot = data
        .limits
        .iter()
        .find(|r| r.setting == "risk.max_daily_loss_total_usd")
        .unwrap();
    assert!(tot.value.is_err() && tot.source == "не задано нигде");

    // Книги: эта и «Итоги сделок» за то же окно; с LibreOffice — итог периода = «Итог факт + оценка».
    let bytes = arb::report::cap_xlsx::workbook(&data, None).unwrap();
    let cx = arb::report::ResultsCtx {
        dex: inputs.rows.dex.clone(),
        snap: inputs.snap.clone(),
        ..Default::default()
    };
    let rbytes = arb::report::results(&deals, w, &cx).unwrap();
    if let Ok(o) = std::env::var("REPORT_CAPITAL_TOTAL_OUT") {
        std::fs::write(o, &bytes).unwrap();
    }
    if let Ok(o) = std::env::var("REPORT_CAPITAL_RESULTS_OUT") {
        std::fs::write(o, &rbytes).unwrap();
    }
    if let (Some(lo), Some(lr)) = (
        recalc(&bytes, &["Итоги", "Сделки"]),
        recalc(&rbytes, &["Итог"]),
    ) {
        let s = &lo["Итоги"];
        let i = s.iter().position(|r| r[0].starts_with("2.")).unwrap();
        let tot = s[i..].iter().find(|r| r[0] == "Итого").unwrap();
        let rr = lr["Итог"]
            .iter()
            .find(|r| r[0] == "Итог факт + оценка, USDT")
            .unwrap();
        near(
            num(&tot[6]).unwrap(),
            num(&rr[1]).unwrap(),
            "LibreOffice: итог периода = «Итоги сделок»",
        );
        near(
            num(&tot[6]).unwrap(),
            theirs,
            "LibreOffice: итог периода = прямой расчёт",
        );
    }
    let _ = std::fs::remove_dir_all(&p);
}
