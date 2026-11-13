//! Файл: tests/core_price_range_268.rs
//! Что делает: решение 268 (Тигран 14.11.2026) на настоящей SQLite во временной папке (ядро без площадок в реестре,
//! моков нет):
//! - ветка ядра (core/sell_range.rs wanted): идёт только при коде PriceOutOfRange, на бирже, при включённой
//!   sell.market_on_price_range (вся система или площадка продажи; значение площадки сильнее) и если у сделки её ещё не
//!   было (Deal.price_range_tried); не задана — нет (поведение до решения);
//! - правило шлюза «Повторить» для PriceOutOfRange не действует (rules_apply::on_error → Stop), счётчик ошибок площадки
//!   подряд не растёт;
//! - п. 6: продажа на бирже остановлена алертом — при sell.block_entry_on_halt = да входы монеты с ПРОДАЖЕЙ на этой
//!   бирже закрыты (запись «sell:<биржа>» в coin.entry_blocked_venues, проверка withdraw_block::check по d.sell_on);
//!   настройка не задана — ничего не пишется; записи решения 253 (покупка на бирже) работают как раньше; DEX — нет;
//! - итог ордера без исполнения «снят защитой цены» → PriceOutOfRange: Gate finish_as = price_protect_cancelled (поля —
//!   по Order.md SDK Gate), Binance status EXPIRED + expiryReason EXECUTION_RULE_PRICE_RANGE_EXCEEDED (поля — по
//!   rest-api.md и enums.md; пример ответа Query order из tests/ioc_263.rs); с исполнением или другой причиной — как
//!   раньше. Живых ответов с этими значениями в tests/data нет — значения полей взяты из официальной документации.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (sell_range, sell_block, withdraw_block, rules_apply, risk), arb::venues::{gate, binance},
//! tests/usdt_return_common.

mod usdt_return_common;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::op_error::OpError;
use arb::core::types::{Asset, Direction, Side, TradeRequest, TradeStatus, VenueId};
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
async fn branch_needs_setting_cex_and_first_time() {
    let d = dir("pr268-wanted");
    let (core, _) = start(&d, None).await;
    kinds(&core);
    let e = mexc_30087();
    assert_eq!(e.code, Some(ErrorCode::PriceOutOfRange));
    let mut deal = doge("20261113211300000FAR");
    let on = arb::core::sell_range::ON;
    // Не задана — ветки нет (поведение до решения).
    assert!(!arb::core::sell_range::wanted(&core, &deal, &e));
    // Вся система — да.
    set(&core, on, Scope::Global, Some(json!(true))).await;
    assert!(arb::core::sell_range::wanted(&core, &deal, &e));
    // Площадка продажи — нет: сильнее общей.
    set(&core, on, Scope::Venue("mexc".into()), Some(json!(false))).await;
    assert!(!arb::core::sell_range::wanted(&core, &deal, &e));
    set(&core, on, Scope::Venue("mexc".into()), Some(json!(true))).await;
    set(&core, on, Scope::Global, None).await;
    assert!(arb::core::sell_range::wanted(&core, &deal, &e));
    // Другой код — нет.
    let other = OpError::from(&GatewayError::new(ErrorCode::OrderRejected, None, "ордер"));
    assert!(!arb::core::sell_range::wanted(&core, &deal, &other));
    // Ветка у сделки уже была — нет (второй отказ — алерт человеку, как раньше).
    deal.price_range_tried = true;
    assert!(!arb::core::sell_range::wanted(&core, &deal, &e));
    deal.price_range_tried = false;
    // Монеты на DEX — нет.
    set(&core, on, Scope::Global, Some(json!(true))).await;
    deal.at = v("bsc_dex");
    assert!(!arb::core::sell_range::wanted(&core, &deal, &e));
    // Старая запись сделки без поля читается: отметки нет.
    let mut old = serde_json::to_value(doge("x")).unwrap();
    old.as_object_mut().unwrap().remove("price_range_tried");
    let back: arb::core::deal::Deal = serde_json::from_value(old).unwrap();
    assert!(!back.price_range_tried);
}

#[tokio::test]
async fn retry_rule_does_not_apply_and_errors_not_counted() {
    use arb::core::rules_apply::{self, Next};
    use arb::gateway::rules::{Rule, RuleAction, RuleScope};
    let d = dir("pr268-rule");
    let (core, j) = start(&d, None).await;
    kinds(&core);
    let rule = |event| Rule {
        id: 0,
        event,
        condition: None,
        action: RuleAction::Retry {
            attempts: 5,
            pause_ms: 1,
        },
        scope: RuleScope::All,
        enabled: true,
    };
    for ev in [ErrorCode::PriceOutOfRange, ErrorCode::RateLimited] {
        arb::storage::rules::create(&core.inner.deps.db, rule(ev))
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

#[tokio::test]
async fn sale_alert_closes_entries_selling_on_that_exchange() {
    let d = dir("pr268-block");
    let (core, j) = start(&d, None).await;
    kinds(&core);
    let deal = doge("20261113211300000FAR");
    let far = deal.direction.asset.clone();
    let dir_of = |buy: &str, sell: &str| Direction {
        asset: far.clone(),
        buy_on: v(buy),
        sell_on: v(sell),
    };
    let e = mexc_30087();
    let check = |d: &Direction| arb::core::withdraw_block::check(&core, d);
    // Настройка не задана — ничего не пишется.
    arb::core::sell_block::close(&core, &deal, &e).await;
    assert!(check(&dir_of("bsc_dex", "mexc")).is_ok());
    assert!(entries(&j, "core.sell_block", "DOGE").await.is_empty());
    // Включена: входы с продажей на mexc закрыты, остальные направления — нет.
    let on = arb::core::sell_block::ON;
    set(&core, on, Scope::Global, Some(json!(true))).await;
    arb::core::sell_block::close(&core, &deal, &e).await;
    let why = check(&dir_of("bsc_dex", "mexc")).unwrap_err();
    assert!(why.contains("входы с продажей на mexc закрыты"), "{why}");
    assert!(
        why.contains("Авто (решение 268): продажа DOGE на mexc остановлена"),
        "{why}"
    );
    assert!(why.contains("Order price exceeds allowed range"), "{why}");
    assert!(why.contains("сделка 20261113211300000FAR"), "{why}");
    assert!(check(&dir_of("bingx", "mexc")).is_err());
    assert!(
        check(&dir_of("mexc", "bsc_dex")).is_ok(),
        "покупка на mexc — открыта"
    );
    assert!(
        check(&dir_of("bsc_dex", "bingx")).is_ok(),
        "продажа на другой бирже — открыта"
    );
    // Повторный алерт той же сделки — без дубля записи и без второго уведомления.
    arb::core::sell_block::close(&core, &deal, &e).await;
    let id = match arb::core::blacklist::coin_scope(&far) {
        Scope::Coin(id) => id,
        _ => unreachable!(),
    };
    let at = arb::settings::read::At {
        venue: None,
        coin: Some(&id),
    };
    let list = |core: &arb::core::Core| {
        core.inner
            .deps
            .settings
            .list(arb::core::withdraw_block::VENUES, at)
            .unwrap_or_default()
    };
    assert_eq!(list(&core), vec!["sell:mexc".to_string()]);
    assert_eq!(entries(&j, "core.sell_block", "DOGE").await.len(), 1);
    // Продажа остановлена на DEX — не записывается.
    let mut on_dex = doge("y");
    on_dex.at = v("bsc_dex");
    arb::core::sell_block::close(&core, &on_dex, &e).await;
    assert_eq!(list(&core), vec!["sell:mexc".to_string()]);
    // Запись решения 253 (отказ вывода с bingx) — как раньше: закрывает покупку на bingx, продажу на bingx — нет.
    let refused = OpError::from(&GatewayError::new(
        ErrorCode::WithdrawRejected,
        Some(v("bingx")),
        "вывод отклонён",
    ));
    arb::core::withdraw_block::close(&core, "z", &far, &v("bingx"), &refused).await;
    assert_eq!(
        list(&core),
        vec!["sell:mexc".to_string(), "bingx".to_string()]
    );
    let why = check(&dir_of("bingx", "bsc_dex")).unwrap_err();
    assert!(
        why.contains("входы с bingx закрыты после отказа вывода"),
        "{why}"
    );
    assert!(check(&dir_of("bsc_dex", "bingx")).is_ok());
    // Человек снял запись продажи (экран «Монеты») — входы с продажей на mexc снова открыты, запись 253 осталась.
    set(
        &core,
        arb::core::withdraw_block::VENUES,
        Scope::Coin(id.clone()),
        Some(json!(["bingx"])),
    )
    .await;
    assert!(check(&dir_of("bsc_dex", "mexc")).is_ok());
    assert!(check(&dir_of("bingx", "bsc_dex")).is_err());
}

/// Ветка без котировки (площадки в реестре нет — котировка честно невозможна): автоматика — режим застрявшей монеты с
/// причиной «нет котировки»; кнопка человека — исходный отказ с причиной (дальше алерт, как раньше). Отметка сделки
/// ставится в обоих случаях — второй раз ветка не идёт; строка — в хронологии и журнале core.sell_range.
#[tokio::test]
async fn branch_without_quote_goes_to_stuck_or_keeps_alert() {
    use arb::core::sell_range::{Outcome, run};
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
    let mut deal = doge("20261113211300000FAR");
    assert!(arb::core::sell_range::wanted(&core, &deal, &e));
    match run(&core, &mut deal, &e, "выгодно").await {
        Outcome::Stuck(StuckReason::NoQuote(_)) => {}
        _ => panic!("ожидался режим застрявшей монеты без котировки"),
    }
    assert!(deal.price_range_tried);
    assert!(!arb::core::sell_range::wanted(&core, &deal, &e));
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
    // Кнопка человека: без котировки рыночный ордер не собрать — исходный отказ (код тот же) с причиной.
    let mut deal = doge("20261113211400000FAR");
    match run(&core, &mut deal, &e, arb::core::sell::BUTTON).await {
        Outcome::Failed(x) => {
            assert_eq!(x.code, Some(ErrorCode::PriceOutOfRange));
            assert!(
                x.message
                    .contains("рыночная продажа невозможна: нет котировки"),
                "{}",
                x.message
            );
        }
        _ => panic!("ожидался исходный отказ"),
    }
    assert!(deal.price_range_tried);
}

fn sell_req(venue: &str) -> TradeRequest {
    TradeRequest {
        venue: v(venue),
        asset: Asset {
            ticker: "FAR".into(),
            bsc_contract: None,
        },
        side: Side::Sell,
        qty: Decimal::from(100),
        worst_price: Decimal::new(46, 5),
        deal_id: "d1".into(),
        route: None,
        exit_position: true,
        remeasure: None,
    }
}

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
    // Gate: поля ордера — Order.md (id, status, amount, filled_amount, filled_total, fee, fee_currency, finish_as).
    let order = |filled: &str, total: &str, finish: &str| {
        json!({"id": "1852454420", "status": "cancelled", "amount": "100", "filled_amount": filled,
               "filled_total": total, "fee": "0", "fee_currency": "USDT", "gt_fee": "0",
               "point_fee": "0", "finish_as": finish})
    };
    let res = |o: &Value| {
        arb::venues::gate::order_state::result(
            &gate,
            sell_req("gate"),
            o,
            "1852454420".into(),
            Decimal::from(100),
        )
    };
    let e = res(&order("0", "0", "price_protect_cancelled")).unwrap_err();
    assert_eq!(e.code, ErrorCode::PriceOutOfRange);
    assert_eq!(e.venue, Some(v("gate")));
    assert!(
        e.message.contains("price_protect_cancelled"),
        "{}",
        e.message
    );
    // Рыночная продажа код снимает.
    assert_eq!(e.no_price_range().code, ErrorCode::OrderRejected);
    // Есть исполнение — обычный частичный итог; другая причина без исполнения — «не исполнился», как раньше.
    let part = res(&order("40", "0.02", "price_protect_cancelled")).unwrap();
    assert_eq!(part.status, TradeStatus::PartiallyFilled);
    assert_eq!(part.filled_qty, Decimal::from(40));
    let ioc = res(&order("0", "0", "ioc")).unwrap();
    assert_eq!(ioc.status, TradeStatus::NotFilled);

    let bn = arb::venues::binance::ctx::Ctx {
        settings: s.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal: core.inner.deps.journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.join("raw-bn"), s),
        taps: Default::default(),
    };
    // Binance: поля — пример ответа Query order (rest-api.md), status и expiryReason — enums.md.
    let order = |status: &str, why: &str, filled: &str, quote: &str| {
        json!({"symbol": "FARUSDT", "orderId": 1, "clientOrderId": "s1", "price": "0.00046",
               "origQty": "100.0", "executedQty": filled, "cummulativeQuoteQty": quote,
               "status": status, "timeInForce": "IOC", "type": "LIMIT", "side": "SELL",
               "expiryReason": why, "fills": []})
    };
    let o = order(
        "EXPIRED",
        "EXECUTION_RULE_PRICE_RANGE_EXCEEDED",
        "0.0",
        "0.0",
    );
    let e = arb::venues::binance::order_state::result(
        &bn,
        "FARUSDT",
        sell_req("binance"),
        &o,
        "1".into(),
        Decimal::from(100),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PriceOutOfRange);
    assert!(
        e.message.contains("EXECUTION_RULE_PRICE_RANGE_EXCEEDED"),
        "{}",
        e.message
    );
    // Другая причина истечения без исполнения — «не исполнился», как раньше.
    let o = order("EXPIRED", "UNFILLED_IOC_QUANTITY_EXPIRED", "0.0", "0.0");
    let r = arb::venues::binance::order_state::result(
        &bn,
        "FARUSDT",
        sell_req("binance"),
        &o,
        "1".into(),
        Decimal::from(100),
    )
    .await
    .unwrap();
    assert_eq!(r.status, TradeStatus::NotFilled);
}
