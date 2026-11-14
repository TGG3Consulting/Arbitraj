//! Файл: tests/core_price_range_268.rs
//! Что делает: решение 268 (Тигран 14.11.2026) на настоящей SQLite во временной папке (ядро без площадок в реестре,
//! моков нет):
//! - ветка ядра (core/sell_range.rs wanted): идёт только при коде PriceOutOfRange, на бирже, при включённой
//!   sell.market_on_price_range (вся система или площадка продажи; значение площадки сильнее) и пока на этой площадке
//!   ветка ещё может что-то сделать (отметки Deal.range: market / stuck — по площадке); не задана — нет;
//!   сделка старого вида (без полей отметок) читается;
//! - правила шлюза: «Повторить» для PriceOutOfRange не действует; своего правила нет — действует правило OrderRejected
//!   (как до решения), запись в журнале об этом; счётчик ошибок площадки подряд не растёт;
//! - ветка без котировки (площадки в реестре нет): обычная продажа — режим застрявшей монеты (отметка по площадке),
//!   режим застрявшей монеты — исходный отказ (алерт), кнопка без худшей цены прежнего ордера — исходный отказ;
//! - итог ордера без исполнения «снят защитой цены»: Gate finish_as = price_protect_cancelled (поля — по Order.md SDK
//!   Gate), Binance status EXPIRED + expiryReason EXECUTION_RULE_PRICE_RANGE_EXCEEDED (поля — по rest-api.md и
//!   enums.md) — код PriceOutOfRange только у ПРОДАЖИ и только при sell.market_on_price_range = да; у покупки и при не
//!   заданной настройке — «не исполнился», как до решения. Живых ответов с этими значениями в tests/data нет —
//!   значения полей взяты из официальной документации.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (sell_range, sell_range_run, rules_apply, risk), arb::venues::{gate, binance},
//! tests/usdt_return_common.

mod usdt_return_common;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::op_error::OpError;
use arb::core::types::{Asset, Side, TradeRequest, TradeStatus, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::settings::value::Scope;
use usdt_return_common::{dir, doge, entries, start};

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

/// Виды площадок в ядре (биржи и bsc_dex).
fn kinds(core: &arb::core::Core) {
    *core.inner.kinds.write().unwrap() = vec![
        (v("bingx"), VenueKind::Cex),
        (v("mexc"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
    ];
}

async fn set(core: &arb::core::Core, name: &str, scope: Scope, val: Option<Value>) {
    core.inner
        .deps
        .settings
        .set(name, scope, val, "тест")
        .await
        .unwrap();
}

/// Отказ MEXC из живого ответа 13.11 (через настоящий разбор адаптера).
fn mexc_30087() -> OpError {
    let raw =
        std::fs::read_to_string("tests/data/mexc_price_range_2026-11-13/order_30087.json").unwrap();
    let e = arb::venues::mexc::errors::from_code(
        30087,
        "Order price exceeds allowed range",
        arb::venues::mexc::errors::Op::Trade,
        raw.trim(),
    );
    OpError::from(&e)
}

#[tokio::test]
async fn branch_needs_setting_cex_and_room_on_venue() {
    use arb::core::sell_range::{Rule, wanted};
    let d = dir("pr268-wanted");
    let (core, _) = start(&d, None).await;
    kinds(&core);
    let e = mexc_30087();
    assert_eq!(e.code, Some(ErrorCode::PriceOutOfRange));
    let mut deal = doge("20261113211300000FAR");
    let on = arb::core::sell_range::ON;
    let mode = Rule::MaxLoss(Decimal::from(5));
    // Не задана — ветки нет (поведение до решения).
    for rule in [Rule::Entry, mode, Rule::Button] {
        assert!(!wanted(&core, &deal, &e, rule));
    }
    // Вся система — да.
    set(&core, on, Scope::Global, Some(json!(true))).await;
    assert!(wanted(&core, &deal, &e, Rule::Entry));
    // Площадка продажи — нет: сильнее общей.
    set(&core, on, Scope::Venue("mexc".into()), Some(json!(false))).await;
    assert!(!wanted(&core, &deal, &e, Rule::Entry));
    set(&core, on, Scope::Venue("mexc".into()), Some(json!(true))).await;
    set(&core, on, Scope::Global, None).await;
    assert!(wanted(&core, &deal, &e, Rule::Entry));
    // Продажа без порога («Ожидание» истекло) — ветки нет.
    assert!(!wanted(&core, &deal, &e, Rule::NoThreshold));
    // Другой код — нет.
    let other = OpError::from(&GatewayError::new(ErrorCode::OrderRejected, None, "ордер"));
    assert!(!wanted(&core, &deal, &other, Rule::Entry));
    // Отметки — по площадке: рыночный на mexc уже отправлялся — режим и кнопка больше не идут, обычная продажа ещё может
    // уйти в режим; уходили и в режим — ветки нет. На другой бирже (после перегона) — своя попытка.
    deal.mark_range_market(v("mexc"));
    assert!(!wanted(&core, &deal, &e, mode) && !wanted(&core, &deal, &e, Rule::Button));
    assert!(wanted(&core, &deal, &e, Rule::Entry));
    deal.mark_range_stuck(v("mexc"));
    assert!(!wanted(&core, &deal, &e, Rule::Entry));
    set(&core, on, Scope::Global, Some(json!(true))).await;
    deal.at = v("bingx");
    for rule in [Rule::Entry, mode, Rule::Button] {
        assert!(wanted(&core, &deal, &e, rule));
    }
    // Монеты на DEX — нет.
    deal.at = v("bsc_dex");
    assert!(!wanted(&core, &deal, &e, Rule::Entry));
    // Запись сделки без полей отметок (до решения) читается; пустые отметки в запись не пишутся.
    let old = serde_json::to_value(doge("x")).unwrap();
    assert!(old.get("range").is_none());
    let back: arb::core::deal::Deal = serde_json::from_value(old).unwrap();
    assert!(back.range_market().is_empty() && back.range_stuck().is_empty());
    let kept: arb::core::deal::Deal =
        serde_json::from_value(serde_json::to_value(&deal).unwrap()).unwrap();
    assert_eq!(
        (kept.range_market().to_vec(), kept.range_stuck().to_vec()),
        (vec![v("mexc")], vec![v("mexc")])
    );
}

#[tokio::test]
async fn retry_rule_does_not_apply_and_errors_not_counted() {
    use arb::core::rules_apply::{self, Next};
    use arb::gateway::rules::{Rule, RuleAction, RuleScope};
    let d = dir("pr268-rule");
    let (core, j) = start(&d, None).await;
    kinds(&core);
    let rule = |event, scope| Rule {
        id: 0,
        event,
        condition: None,
        action: RuleAction::Retry {
            attempts: 5,
            pause_ms: 1,
        },
        scope,
        enabled: true,
    };
    // Своё правило PriceOutOfRange — только у mexc.
    for (ev, scope) in [
        (ErrorCode::PriceOutOfRange, RuleScope::Venue(v("mexc"))),
        (ErrorCode::RateLimited, RuleScope::All),
    ] {
        arb::storage::rules::create(&core.inner.deps.db, rule(ev, scope))
            .await
            .unwrap();
    }
    set(
        &core,
        "risk.max_errors_in_row",
        Scope::Venue("mexc".into()),
        Some(json!(1)),
    )
    .await;
    let e = GatewayError::new(
        ErrorCode::PriceOutOfRange,
        Some(v("mexc")),
        "MEXC, код 30087: Order price exceeds allowed range",
    );
    for attempt in 1..=3 {
        assert_eq!(
            rules_apply::on_error(&core, "Продажа", &v("mexc"), Some("d1"), &e, attempt).await,
            Next::Stop
        );
    }
    // Ревью A2: своего правила нет, а для OrderRejected задано «Стоп площадка» — действует оно (как до решения 268),
    // в журнале — честная запись; на gate правил нет — «правила нет».
    let stop = Rule {
        id: 0,
        event: ErrorCode::OrderRejected,
        condition: None,
        action: RuleAction::StopVenue,
        scope: RuleScope::Venue(v("bingx")),
        enabled: true,
    };
    arb::storage::rules::create(&core.inner.deps.db, stop)
        .await
        .unwrap();
    let on_bingx = GatewayError::new(
        ErrorCode::PriceOutOfRange,
        Some(v("bingx")),
        "BingX, код 100440: x",
    );
    assert!(
        entries(&j, "core.rules", "применено правило шлюза")
            .await
            .is_empty(),
        "у mexc — своё правило"
    );
    assert_eq!(
        rules_apply::on_error(&core, "Продажа", &v("bingx"), Some("d2"), &on_bingx, 1).await,
        Next::Stop
    );
    assert_eq!(
        entries(
            &j,
            "core.rules",
            "Правила для PriceOutOfRange нет — применено правило шлюза"
        )
        .await
        .len(),
        1
    );
    assert!(
        core.inner
            .risk
            .lock()
            .unwrap()
            .venue_paused
            .contains_key(&v("bingx")),
        "правило OrderRejected «Стоп площадка» сработало"
    );
    // Ошибкой площадки подряд не считается: паузы площадки нет.
    assert!(
        !core
            .inner
            .risk
            .lock()
            .unwrap()
            .venue_paused
            .contains_key(&v("mexc")),
        "три отказа подряд при risk.max_errors_in_row = 1 — паузы нет"
    );
    // То же правило у другого кода действует — запрет именно по коду.
    let rl = GatewayError::new(ErrorCode::RateLimited, Some(v("mexc")), "429");
    assert_eq!(
        rules_apply::on_error(&core, "Продажа", &v("mexc"), Some("d1"), &rl, 1).await,
        Next::Retry { pause_ms: 1 }
    );
    assert!(!entries(&j, "core.op_error", "30087").await.is_empty());
}

/// Ветка без котировки (площадки в реестре нет — котировка честно невозможна): обычная продажа — режим застрявшей
/// монеты с причиной «нет котировки», отметка «уходили в режим» — по площадке; режим застрявшей монеты — исходный
/// отказ с причиной (дальше алерт, как раньше), отметок нет; кнопка, у которой нет и худшей цены прежнего ордера, —
/// исходный отказ. Строка — в хронологии и журнале core.sell_range.
#[tokio::test]
async fn branch_without_quote() {
    use arb::core::sell_range::{Outcome, Rule};
    use arb::core::sell_range_run::{Prev, branch, run};
    use arb::core::stuck_reason::StuckReason;
    let d = dir("pr268-run");
    let (core, j) = start(&d, None).await;
    kinds(&core);
    set(
        &core,
        arb::core::sell_range::ON,
        Scope::Global,
        Some(json!(true)),
    )
    .await;
    let e = mexc_30087();
    let q = arb::core::types::Quote {
        venue: v("mexc"),
        asset: doge("x").direction.asset,
        side: Side::Sell,
        qty: Decimal::ONE,
        fillable_qty: Decimal::ONE,
        avg_price: Decimal::ONE,
        costs: vec![],
        route: None,
        signal_route: None,
        pick: None,
        at: chrono::Utc::now(),
    };
    let prev = Prev { q: &q, worst: None };
    let mut deal = doge("20261113211300000FAR");
    match branch(&core, &mut deal, &e, "выгодно", Rule::Entry, prev).await {
        Outcome::Stuck(StuckReason::NoQuote(_)) => {}
        _ => panic!("ожидался режим застрявшей монеты без котировки"),
    }
    assert_eq!(deal.range_stuck(), vec![v("mexc")]);
    assert!(deal.range_market().is_empty());
    let last = &deal.timeline.last().unwrap().1;
    assert!(
        last.starts_with("Биржа отклонила лимитный ордер продажи: цена вне разрешённого диапазона (MEXC, код 30087: Order price exceeds allowed range)"),
        "{last}"
    );
    assert!(
        last.contains("режим застрявшей монеты (решение 268)"),
        "{last}"
    );
    assert!(!entries(&j, "core.sell_range", &deal.id).await.is_empty());
    // Режим застрявшей монеты и кнопка без худшей цены прежнего ордера: исходный отказ (код тот же) с причиной —
    // он же остаётся в итоге продажи (run: false — в режим не уходили).
    for (rule, why) in [
        (Rule::MaxLoss(Decimal::from(5)), "убыток в пороге"),
        (Rule::Button, arb::core::sell::BUTTON),
    ] {
        let mut deal = doge("20261113211400000FAR");
        let mut res = Err(e.clone());
        assert!(!run(&core, &mut deal, &mut res, why, rule, &q, &None).await);
        let x = res.unwrap_err();
        assert_eq!(x.code, Some(ErrorCode::PriceOutOfRange));
        assert!(
            x.message
                .contains("рыночная продажа невозможна: нет котировки"),
            "{}",
            x.message
        );
        assert!(deal.range_market().is_empty() && deal.range_stuck().is_empty());
    }
}

fn req(venue: &str, side: Side) -> TradeRequest {
    TradeRequest {
        venue: v(venue),
        asset: Asset {
            ticker: "FAR".into(),
            bsc_contract: None,
        },
        side,
        qty: Decimal::from(100),
        worst_price: Decimal::new(46, 5),
        deal_id: "d1".into(),
        route: None,
        exit_position: side == Side::Sell,
        remeasure: None,
    }
}

/// Итог ордера «снят защитой цены без исполнения»: PriceOutOfRange — только у продажи и только при включённой
/// sell.market_on_price_range (площадка или вся система); иначе — «не исполнился», как до решения (ef2c8a4).
#[tokio::test]
async fn gate_and_binance_order_cancelled_by_price_protection() {
    let d = dir("pr268-state");
    let (core, _) = start(&d, None).await;
    let s = core.inner.deps.settings.clone();
    let gate = arb::venues::gate::ctx::Ctx {
        settings: s.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal: core.inner.deps.journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.join("raw-gate"), s.clone()),
        taps: Default::default(),
    };
    let bn = arb::venues::binance::ctx::Ctx {
        settings: s.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal: core.inner.deps.journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.join("raw-bn"), s),
        taps: Default::default(),
    };
    // Gate: поля ордера — Order.md (id, status, amount, filled_amount, filled_total, fee, fee_currency, finish_as).
    let g_order = |filled: &str, total: &str, finish: &str| {
        json!({"id": "1852454420", "status": "cancelled", "amount": "100", "filled_amount": filled,
               "filled_total": total, "fee": "0", "fee_currency": "USDT", "gt_fee": "0",
               "point_fee": "0", "finish_as": finish})
    };
    let g = |o: &Value, side| {
        arb::venues::gate::order_state::result(
            &gate,
            req("gate", side),
            o,
            "1852454420".into(),
            Decimal::from(100),
        )
    };
    // Binance: поля — пример ответа Query order (rest-api.md), status и expiryReason — enums.md.
    let b_order = |why: &str, filled: &str| {
        json!({"symbol": "FARUSDT", "orderId": 1, "clientOrderId": "s1", "price": "0.00046",
               "origQty": "100.0", "executedQty": filled, "cummulativeQuoteQty": "0.0",
               "status": "EXPIRED", "timeInForce": "IOC", "type": "LIMIT", "side": "SELL",
               "expiryReason": why, "fills": []})
    };
    let range = "EXECUTION_RULE_PRICE_RANGE_EXCEEDED";
    let g_cut = g_order("0", "0", "price_protect_cancelled");
    let b_cut = b_order(range, "0.0");
    // Настройка не задана — как до решения: «не исполнился» и у продажи, и у покупки.
    for side in [Side::Sell, Side::Buy] {
        assert_eq!(g(&g_cut, side).unwrap().status, TradeStatus::NotFilled);
        let r = arb::venues::binance::order_state::result(
            &bn,
            "FARUSDT",
            req("binance", side),
            &b_cut,
            "1".into(),
            Decimal::from(100),
        )
        .await
        .unwrap();
        assert_eq!(r.status, TradeStatus::NotFilled);
    }
    // Включена (вся система): у продажи — PriceOutOfRange, у покупки — по-прежнему «не исполнился».
    set(
        &core,
        arb::core::sell_range::ON,
        Scope::Global,
        Some(json!(true)),
    )
    .await;
    let e = g(&g_cut, Side::Sell).unwrap_err();
    assert_eq!(e.code, ErrorCode::PriceOutOfRange);
    assert_eq!(e.venue, Some(v("gate")));
    assert!(
        e.message.contains("price_protect_cancelled"),
        "{}",
        e.message
    );
    assert_eq!(
        e.no_price_range().code,
        ErrorCode::OrderRejected,
        "рыночная продажа код снимает"
    );
    assert_eq!(g(&g_cut, Side::Buy).unwrap().status, TradeStatus::NotFilled);
    let e = arb::venues::binance::order_state::result(
        &bn,
        "FARUSDT",
        req("binance", Side::Sell),
        &b_cut,
        "1".into(),
        Decimal::from(100),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PriceOutOfRange);
    assert!(e.message.contains(range), "{}", e.message);
    let r = arb::venues::binance::order_state::result(
        &bn,
        "FARUSDT",
        req("binance", Side::Buy),
        &b_cut,
        "1".into(),
        Decimal::from(100),
    )
    .await
    .unwrap();
    assert_eq!(r.status, TradeStatus::NotFilled);
    // Площадка — нет: сильнее общей.
    set(
        &core,
        arb::core::sell_range::ON,
        Scope::Venue("gate".into()),
        Some(json!(false)),
    )
    .await;
    assert_eq!(
        g(&g_cut, Side::Sell).unwrap().status,
        TradeStatus::NotFilled
    );
    set(
        &core,
        arb::core::sell_range::ON,
        Scope::Venue("gate".into()),
        None,
    )
    .await;
    // Есть исполнение — обычный частичный итог; другая причина без исполнения — «не исполнился».
    let part = g(
        &g_order("40", "0.02", "price_protect_cancelled"),
        Side::Sell,
    )
    .unwrap();
    assert_eq!(part.status, TradeStatus::PartiallyFilled);
    assert_eq!(part.filled_qty, Decimal::from(40));
    assert_eq!(
        g(&g_order("0", "0", "ioc"), Side::Sell).unwrap().status,
        TradeStatus::NotFilled
    );
    let other = b_order("UNFILLED_IOC_QUANTITY_EXPIRED", "0.0");
    let r = arb::venues::binance::order_state::result(
        &bn,
        "FARUSDT",
        req("binance", Side::Sell),
        &other,
        "1".into(),
        Decimal::from(100),
    )
    .await
    .unwrap();
    assert_eq!(r.status, TradeStatus::NotFilled);
}
