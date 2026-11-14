//! Файл: tests/price_range_268.rs
//! Что делает: решение 268 (Тигран 14.11.2026) — чистые функции:
//! - сопоставление кодов бирж «цена лимитного ордера вне диапазона биржи» с кодом шлюза PriceOutOfRange (тексты — из
//!   постановки решения: официальные таблицы ошибок бирж; MEXC 30087 — живой ответ 13.11.2026,
//!   tests/data/mexc_price_range_2026-11-13/order_30087.json); соседние коды, которые сопоставлять НЕЛЬЗЯ, остаются
//!   обычным отказом; KuCoin, Kraken, Phemex — не сопоставлены;
//! - ошибка рыночного ордера никогда не несёт PriceOutOfRange (GatewayError::no_price_range);
//! - тело / параметры запроса рыночной продажи у каждого из 14 адаптеров (venues/<биржа>/market_sell.rs) — поля по
//!   постановке решения (официальная документация бирж);
//! - решение ядра (core/sell_range.rs decide): код, биржа / DEX, настройка, условие продажи (обычная / порог режима /
//!   без порога / кнопка), отметки по площадке, свежая котировка → рынок / застрявшая монета / как раньше (алерт),
//!   включая «сначала невыгодно → режим → снова отказ → в пороге → рыночный» и «… → выше порога → алерт»;
//! - MEXC 30087 при HTTP 5XX — не отказ по цене, а «итог неизвестен»; правило шлюза для PriceOutOfRange — своё или,
//!   если его нет, правило OrderRejected;
//! - правило шлюза «Повторить» для нового кода запрещено; ошибкой площадки подряд не считается; код есть в справочнике
//!   формы правил; человеческая причина в словаре уведомлений.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::*, arb::gateway::{errors, rules}, arb::core::{sell_range, risk_count,
//! outcome_resolve}, arb::api::rules_codes, arb::notify::reason. Моков нет — чистые функции.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::sell_range::{Act, Fresh, Marks, Rule, applies, decide, holds, possible, rule_of};
use arb::gateway::errors::{ErrorCode as C, GatewayError};
use arb::venues::{
    binance, bingx, bitget, bybit, coinex, gate, htx, kraken, kucoin, mexc, okx, phemex, poloniex,
    xt,
};

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

#[test]
fn mexc_recorded_30087_is_price_out_of_range() {
    let raw =
        std::fs::read_to_string("tests/data/mexc_price_range_2026-11-13/order_30087.json").unwrap();
    let v: Value = serde_json::from_str(raw.trim()).unwrap();
    let (code, msg) = (v["code"].as_i64().unwrap(), v["msg"].as_str().unwrap());
    assert_eq!((code, msg), (30087, "Order price exceeds allowed range"));
    // Настоящий ордер и пробный ордер (Op::Other) распознают одинаково.
    for op in [mexc::errors::Op::Trade, mexc::errors::Op::Other] {
        let e = mexc::errors::from_code(code, msg, op, raw.trim());
        assert_eq!(e.code, C::PriceOutOfRange, "{op:?}");
        assert_eq!(
            e.message,
            "MEXC, код 30087: Order price exceeds allowed range"
        );
        assert_eq!(e.raw.as_deref(), Some(raw.trim()));
    }
    // Ревью A1: тот же код при HTTP 5XX («issue on the MEXC server side», решение 263) — не отказ по цене, а сбой
    // сервера: у ордера Timeout (→ «итог неизвестен»), у прочего — Unreachable.
    use mexc::errors::{Op as M, from_code_http};
    let at = |op, st| from_code_http(code, msg, op, raw.trim(), st).code;
    assert_eq!(at(M::Trade, Some(500)), C::Timeout);
    assert_eq!(at(M::Trade, Some(503)), C::Timeout);
    assert_eq!(at(M::Other, Some(502)), C::Unreachable);
    assert_eq!(at(M::Trade, Some(400)), C::PriceOutOfRange);
    assert_eq!(at(M::Trade, Some(200)), C::PriceOutOfRange);
    assert_eq!(
        mexc::errors::unsure(
            from_code_http(code, msg, M::Trade, raw.trim(), Some(500)),
            M::Trade
        )
        .code,
        C::OutcomeUnknown
    );
    // Отказы рыночного ордера — обычный отказ.
    for (c, m) in [
        (30018, "market order is disabled"),
        (30019, "api market order is disabled"),
        (30041, "(без msg)"),
    ] {
        let e = mexc::errors::from_code(c, m, mexc::errors::Op::Trade, "{}");
        assert_eq!(e.code, C::OrderRejected, "{c}");
    }
}

#[test]
fn bingx_binance_bybit_codes() {
    let m = "Order price deviates too much from market price, triggering price protection. Adjust the price.";
    for op in [bingx::errors::Op::Trade, bingx::errors::Op::Other] {
        assert_eq!(
            bingx::errors::from_code(100440, m, op, "{}").code,
            C::PriceOutOfRange
        );
    }
    use binance::errors::{Op as B, from_code as bn};
    for m in [
        "Filter failure: PERCENT_PRICE",
        "Filter failure: PERCENT_PRICE_BY_SIDE",
    ] {
        assert_eq!(bn(-1013, m, B::Trade, "{}").code, C::PriceOutOfRange, "{m}");
        assert_eq!(bn(-1013, m, B::Other, "{}").code, C::PriceOutOfRange, "{m}");
    }
    // Другие −1013 — как раньше.
    for m in ["Filter failure: LOT_SIZE", "Filter failure: NOTIONAL", ""] {
        assert_eq!(bn(-1013, m, B::Trade, "{}").code, C::OrderRejected, "{m}");
    }
    for (c, m) in [
        (170193, "Buy order price cannot be higher than %s."),
        (170194, "Sell order price cannot be lower than %s."),
        (170192, "Order price cannot be higher than %s ."),
        (170371, "the price limitation"),
        (170372, "the price limitation"),
    ] {
        let e = bybit::errors::from_code(c, m, bybit::errors::Op::Trade, "{}");
        assert_eq!(e.code, C::PriceOutOfRange, "{c}");
    }
    // Соседний код Bybit — обычный отказ.
    let e = bybit::errors::from_code(170136, "x", bybit::errors::Op::Trade, "{}");
    assert_eq!(e.code, C::OrderRejected);
}

#[test]
fn xt_poloniex_okx_htx_coinex_bitget_codes() {
    for mc in [
        "ORDER_F0501",
        "ORDER_F0502",
        "ORDER_F0503",
        "ORDER_F0504",
        "ORDER_F0401",
    ] {
        let e = xt::errors::from_code(mc, "[]", xt::errors::Op::Trade, "{}");
        assert_eq!(e.code, C::PriceOutOfRange, "{mc}");
    }
    // «Trigger PROTECTION_MARKET Filter» (рыночный ордер) — обычный отказ: иначе повтор по кругу.
    let e = xt::errors::from_code("ORDER_F0601", "[]", xt::errors::Op::Trade, "{}");
    assert_eq!(e.code, C::OrderRejected);

    use poloniex::errors::{Op as P, from_code as polo};
    for (c, m) in [
        ("21310", "Order price must be less than max price"),
        ("21311", "Order price must be greater than min price"),
    ] {
        assert_eq!(polo(400, c, m, P::Trade, "{}").code, C::PriceOutOfRange);
    }
    assert_eq!(
        polo(400, "21356", "x", P::Trade, "{}").code,
        C::OrderRejected
    );

    for (c, m) in [
        (
            "51006",
            "Order price is not within the price limit (max buy price…, min sell price…)",
        ),
        (
            "51137",
            "The highest price limit for buy orders is {param0}.",
        ),
        (
            "51138",
            "The lowest price limit for sell orders is {param0}.",
        ),
    ] {
        let e = okx::errors::from_code(c, m, okx::errors::Op::Trade, "{}");
        assert_eq!(e.code, C::PriceOutOfRange, "{c}");
    }
    assert_eq!(
        okx::errors::from_code("51000", "x", okx::errors::Op::Trade, "{}").code,
        C::OrderRejected
    );

    use htx::errors::{Op as H, from_code as hx};
    for (c, m) in [
        (
            "order-limitorder-price-min-error",
            "Sell price cannot be lower than specific price",
        ),
        (
            "order-limitorder-price-max-error",
            "Buy price cannot be higher than specific price",
        ),
    ] {
        assert_eq!(
            hx(200, c, m, H::Trade, "{}").code,
            C::PriceOutOfRange,
            "{c}"
        );
    }
    // «before market opens» и order-invalid-price — НЕ сопоставляются.
    for c in [
        "order-price-greater-than-limit",
        "order-price-less-than-limit",
        "order-invalid-price",
    ] {
        assert_eq!(
            hx(200, c, "x", H::Trade, "{}").code,
            C::OrderRejected,
            "{c}"
        );
    }

    for c in ["3606", "3615", "3618", "3616", "3632", "3617", "3633"] {
        let e = coinex::errors::from_code(c, "x", coinex::errors::Op::Trade, "{}");
        assert_eq!(e.code, C::PriceOutOfRange, "{c}");
    }
    assert_eq!(
        coinex::errors::from_code("3600", "x", coinex::errors::Op::Trade, "{}").code,
        C::OrderRejected
    );

    for c in ["43008", "43009", "43039", "43040", "40815", "40816"] {
        let e = bitget::errors::from_code(400, c, "x", bitget::errors::Op::Trade, "{}");
        assert_eq!(e.code, C::PriceOutOfRange, "{c}");
    }
    assert_eq!(
        bitget::errors::from_code(400, "43012", "x", bitget::errors::Op::Trade, "{}").code,
        C::OrderRejected
    );
}

#[test]
fn market_order_error_never_carries_the_new_code() {
    let e = GatewayError::new(C::PriceOutOfRange, None, "x").with_raw("{}");
    let e = e.no_price_range();
    assert_eq!(e.code, C::OrderRejected);
    assert_eq!((e.message.as_str(), e.raw.as_deref()), ("x", Some("{}")));
    // Прочие коды не трогаются.
    for c in [C::OrderNotFilled, C::OutcomeUnknown, C::InsufficientBalance] {
        assert_eq!(GatewayError::new(c, None, "x").no_price_range().code, c);
    }
}

fn pairs<'a>(p: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    p.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

#[test]
fn market_sell_requests_by_exchange() {
    let q = d("12345.6700");
    assert_eq!(
        pairs(&mexc::market_sell::params("FARUSDT", q, "s1_d")),
        vec![
            ("symbol", "FARUSDT"),
            ("side", "SELL"),
            ("type", "MARKET"),
            ("quantity", "12345.67"),
            ("newClientOrderId", "s1_d"),
        ]
    );
    assert_eq!(
        pairs(&bingx::market_sell::params("FAR-USDT", q, "s1_d")),
        vec![
            ("symbol", "FAR-USDT"),
            ("side", "SELL"),
            ("type", "MARKET"),
            ("quantity", "12345.67"),
            ("newClientOrderId", "s1_d"),
        ]
    );
    assert_eq!(
        pairs(&binance::market_sell::params("FARUSDT", q, "s1_d")),
        vec![
            ("symbol", "FARUSDT"),
            ("side", "SELL"),
            ("type", "MARKET"),
            ("quantity", "12345.67"),
            ("newClientOrderId", "s1_d"),
            ("newOrderRespType", "FULL"),
        ]
    );
    assert_eq!(
        gate::market_sell::body("FAR_USDT", q, "t-s1"),
        json!({"text": "t-s1", "currency_pair": "FAR_USDT", "type": "market", "account": "spot",
               "side": "sell", "amount": "12345.67", "time_in_force": "ioc"})
    );
    assert_eq!(
        bitget::market_sell::body("FARUSDT", q, "ioc", "s1"),
        json!({"symbol": "FARUSDT", "side": "sell", "orderType": "market", "force": "ioc",
               "size": "12345.67", "clientOid": "s1"})
    );
    assert_eq!(
        kucoin::market_sell::body("FAR-USDT", q, "s1"),
        json!({"clientOid": "s1", "side": "sell", "symbol": "FAR-USDT", "type": "market",
               "size": "12345.67"})
    );
    assert_eq!(
        xt::market_sell::body("far_usdt", q, "s1"),
        json!({"symbol": "far_usdt", "side": "SELL", "type": "MARKET", "timeInForce": "IOC",
               "bizType": "SPOT", "quantity": 12345.67, "clientOrderId": "s1"})
    );
    assert_eq!(
        bybit::market_sell::body("FARUSDT", q, "s1"),
        json!({"category": "spot", "symbol": "FARUSDT", "isLeverage": 0, "side": "Sell",
               "orderType": "Market", "qty": "12345.67", "marketUnit": "baseCoin",
               "orderLinkId": "s1"})
    );
    assert_eq!(
        poloniex::market_sell::body("FAR_USDT", q, "s1"),
        json!({"symbol": "FAR_USDT", "side": "SELL", "type": "MARKET", "quantity": "12345.67",
               "clientOrderId": "s1", "allowBorrow": false})
    );
    assert_eq!(
        okx::market_sell::body("FAR-USDT", q, "s1"),
        json!({"instId": "FAR-USDT", "tdMode": "cash", "clOrdId": "s1", "side": "sell",
               "ordType": "market", "sz": "12345.67"})
    );
    assert_eq!(
        htx::market_sell::body(42, "farusdt", q, "s1"),
        json!({"account-id": "42", "symbol": "farusdt", "type": "sell-market",
               "amount": "12345.67", "source": "spot-api", "client-order-id": "s1"})
    );
    let kr: Vec<(String, String)> = kraken::market_sell::form("FARUSD", q, "s1");
    let kr: Vec<(&str, &str)> = kr.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        kr,
        vec![
            ("ordertype", "market"),
            ("type", "sell"),
            ("pair", "FARUSD"),
            ("volume", "12345.67"),
            ("oflags", "fciq"),
            ("cl_ord_id", "s1"),
        ]
    );
    assert_eq!(
        coinex::market_sell::body("FARUSDT", "FAR", q, "s1"),
        json!({"market": "FARUSDT", "market_type": "SPOT", "side": "sell", "type": "market",
               "amount": "12345.67", "ccy": "FAR", "client_id": "s1"})
    );
    assert_eq!(
        pairs(&phemex::market_sell::query(
            "sFARUSDT",
            "uuid",
            1_234_567_000_000
        )),
        vec![
            ("symbol", "sFARUSDT"),
            ("clOrdID", "uuid"),
            ("side", "Sell"),
            ("qtyType", "ByBase"),
            ("baseQtyEv", "1234567000000"),
            ("ordType", "Market"),
        ]
    );
    // Ни в одном рыночном запросе нет цены.
    for b in [
        gate::market_sell::body("A_B", q, "t"),
        bitget::market_sell::body("AB", q, "ioc", "c"),
        kucoin::market_sell::body("A-B", q, "c"),
        xt::market_sell::body("a_b", q, "c"),
        bybit::market_sell::body("AB", q, "c"),
        poloniex::market_sell::body("A_B", q, "c"),
        okx::market_sell::body("A-B", q, "c"),
        htx::market_sell::body(1, "ab", q, "c"),
        coinex::market_sell::body("AB", "A", q, "c"),
    ] {
        let o = b.as_object().unwrap();
        for k in ["price", "px", "priceEp"] {
            assert!(!o.contains_key(k), "{b}");
        }
    }
}

fn fresh(profitable: bool, loss: &str) -> Option<Fresh> {
    Some(Fresh {
        profitable,
        loss_pct: d(loss),
    })
}

const NONE: Marks = Marks {
    market: false,
    stuck: false,
};
const SENT: Marks = Marks {
    market: true,
    stuck: false,
};
const MODE: Marks = Marks {
    market: false,
    stuck: true,
};
const BOTH: Marks = Marks {
    market: true,
    stuck: true,
};

#[test]
fn core_decision_table() {
    let code = Some(C::PriceOutOfRange);
    let t = |rule, m, f| decide(code, false, true, true, rule, m, f);
    let (good, bad) = (fresh(true, "-3"), fresh(false, "4"));
    // Обычная продажа: выгодно — рыночный; невыгодно или нет котировки — режим застрявшей монеты.
    assert_eq!(t(Rule::Entry, NONE, good), Act::Market);
    assert_eq!(t(Rule::Entry, NONE, bad), Act::Stuck);
    assert_eq!(t(Rule::Entry, NONE, None), Act::Stuck);
    // В режим из-за коридора на этой площадке уже уходили: выгодно — рыночный (ещё не отправлялся); иначе алерт.
    assert_eq!(t(Rule::Entry, MODE, good), Act::Market);
    assert_eq!(t(Rule::Entry, MODE, bad), Act::Keep);
    assert_eq!(t(Rule::Entry, MODE, None), Act::Keep);
    // Рыночный на этой площадке уже отправлялся: второго нет; невыгодно — в режим (если ещё не уходили).
    assert_eq!(t(Rule::Entry, SENT, good), Act::Keep);
    assert_eq!(t(Rule::Entry, SENT, bad), Act::Stuck);
    assert_eq!(t(Rule::Entry, BOTH, good), Act::Keep);
    assert_eq!(t(Rule::Entry, BOTH, bad), Act::Keep);
    // Режим застрявшей монеты решил продать здесь: то же условие режима на свежей котировке.
    let five = Rule::MaxLoss(d("5"));
    assert_eq!(t(five, NONE, bad), Act::Market, "убыток 4% при пороге 5%");
    assert_eq!(t(five, NONE, fresh(false, "5")), Act::Market, "ровно порог");
    assert_eq!(
        t(five, NONE, fresh(false, "5.01")),
        Act::Keep,
        "выше порога — алерт"
    );
    assert_eq!(t(five, NONE, None), Act::Keep, "нет котировки — алерт");
    assert_eq!(
        t(five, MODE, bad),
        Act::Market,
        "сначала невыгодно → режим → отказ → в пороге → рыночный"
    );
    assert_eq!(
        t(five, MODE, fresh(false, "9")),
        Act::Keep,
        "… → выше порога → алерт"
    );
    assert_eq!(t(five, SENT, bad), Act::Keep, "рыночный уже отправлялся");
    // «Ожидание: стало выгодно» — порог 0: условие «выгодно».
    let zero = Rule::MaxLoss(Decimal::ZERO);
    assert_eq!(t(zero, NONE, good), Act::Market);
    assert_eq!(
        t(zero, NONE, fresh(false, "0")),
        Act::Keep,
        "невыгодно при убытке 0% — не «выгодно»"
    );
    assert!(holds(Decimal::ZERO, fresh(true, "0").unwrap()));
    // Продажа без порога («Ожидание» истекло, частичная покупка) — рыночного нет никогда.
    for f in [good, bad, None] {
        assert_eq!(t(Rule::NoThreshold, NONE, f), Act::Keep);
    }
    // Кнопка человека — без проверки условия и даже без котировки; второй раз без нового нажатия — нет.
    for f in [good, bad, None] {
        assert_eq!(t(Rule::Button, NONE, f), Act::Market);
        assert_eq!(t(Rule::Button, MODE, f), Act::Market);
        assert_eq!(t(Rule::Button, SENT, f), Act::Keep);
    }
    // Настройка выключена, DEX, ордер ушёл в сеть, другой код — как раньше при любом условии.
    for rule in [Rule::Entry, five, Rule::Button] {
        assert_eq!(
            decide(code, false, true, false, rule, NONE, good),
            Act::Keep
        );
        assert_eq!(
            decide(code, false, false, true, rule, NONE, good),
            Act::Keep
        );
        assert_eq!(decide(code, true, true, true, rule, NONE, good), Act::Keep);
        for other in [
            Some(C::OrderRejected),
            Some(C::OrderNotFilled),
            Some(C::OutcomeUnknown),
            None,
        ] {
            assert_eq!(
                decide(other, false, true, true, rule, NONE, good),
                Act::Keep
            );
            assert!(!applies(other, false, true, true));
        }
    }
    assert!(applies(code, false, true, true));
    // Котировка берётся, только если ветка ещё может что-то сделать.
    assert!(
        possible(Rule::Entry, NONE) && possible(Rule::Entry, SENT) && possible(Rule::Entry, MODE)
    );
    assert!(!possible(Rule::Entry, BOTH));
    assert!(possible(five, MODE) && !possible(five, SENT));
    assert!(possible(Rule::Button, NONE) && !possible(Rule::Button, SENT));
    assert!(!possible(Rule::NoThreshold, NONE));
}

/// Условие продажи по причине и порогу, с которыми её запускают sell::market_guarded (stuck.rs, relay_here.rs,
/// executor.rs, кнопка).
#[test]
fn rule_by_reason_and_guard() {
    use arb::core::remeasure::{Guard, PROFITABLE, PROFITABLE_OPEN};
    let button = arb::core::sell::BUTTON;
    assert_eq!(rule_of(button, Guard::Any), Rule::Button);
    assert_eq!(
        rule_of("порог", Guard::MaxLoss(d("5"))),
        Rule::MaxLoss(d("5"))
    );
    assert_eq!(
        rule_of("ожидание: стало выгодно", PROFITABLE),
        Rule::MaxLoss(Decimal::ZERO)
    );
    // «Ожидание» по истечении и частичная покупка (PROFITABLE_OPEN) — продажа без порога.
    assert_eq!(
        rule_of("ждали — выгодно не стало", PROFITABLE_OPEN),
        Rule::NoThreshold
    );
    assert_eq!(rule_of("x", Guard::Off), Rule::NoThreshold);
}

#[test]
fn rule_retry_forbidden_not_counted_listed() {
    use arb::gateway::rules::{inactive_reason, price_range, retry_allowed, withdraw_refusal};
    assert!(price_range(C::PriceOutOfRange));
    assert!(!price_range(C::OrderRejected));
    let why = inactive_reason(C::PriceOutOfRange, "Retry").expect("повтор запрещён");
    assert!(why.contains("решение 268"), "{why}");
    for kind in ["StopVenue", "StopAll", "CancelNoRetry", "NotifyOnly"] {
        assert_eq!(inactive_reason(C::PriceOutOfRange, kind), None, "{kind}");
    }
    // Ревью A2: своего правила нет — действует правило OrderRejected (площадки, затем общее); своё — сильнее.
    use arb::core::types::VenueId;
    use arb::gateway::rules::{Rule as GwRule, RuleAction, RuleScope, pick_rule_or_fallback};
    let rule = |id, event, scope| GwRule {
        id,
        event,
        condition: None,
        action: RuleAction::StopVenue,
        scope,
        enabled: true,
    };
    let v = VenueId("mexc".into());
    let mut rules = vec![
        rule(1, C::OrderRejected, RuleScope::All),
        rule(2, C::OrderRejected, RuleScope::Venue(v.clone())),
    ];
    let got = pick_rule_or_fallback(&rules, C::PriceOutOfRange, Some(&v)).unwrap();
    assert_eq!((got.0.id, got.1), (2, true));
    let other = VenueId("gate".into());
    let got = pick_rule_or_fallback(&rules, C::PriceOutOfRange, Some(&other)).unwrap();
    assert_eq!((got.0.id, got.1), (1, true));
    rules.push(rule(3, C::PriceOutOfRange, RuleScope::All));
    let got = pick_rule_or_fallback(&rules, C::PriceOutOfRange, Some(&v)).unwrap();
    assert_eq!((got.0.id, got.1), (3, false), "своё правило сильнее");
    // Запасного кода у других ошибок нет.
    assert!(pick_rule_or_fallback(&rules, C::OrderNotFilled, Some(&v)).is_none());
    assert_eq!(
        pick_rule_or_fallback(&rules, C::OrderRejected, Some(&v))
            .unwrap()
            .1,
        false
    );
    // Не «исход неизвестен» и не отказ вывода: пауз «только человек» и запрета пауз нет.
    assert!(retry_allowed(C::PriceOutOfRange));
    assert!(!withdraw_refusal(C::PriceOutOfRange));
    // Прежний OrderRejected — без изменений.
    assert_eq!(inactive_reason(C::OrderRejected, "Retry"), None);
    // Не ошибка площадки подряд (ордер не принят, деньги не тронуты).
    let e = GatewayError::new(C::PriceOutOfRange, None, "x");
    assert!(!arb::core::risk_count::counts(&e));
    // Найденный ботом ордер с таким отказом — окончательный отказ (ничего не исполнено).
    assert!(arb::core::outcome_resolve::definitive(&e));
    // Справочник формы правил.
    let codes = arb::api::rules_codes::codes();
    let m = codes
        .iter()
        .find(|c| c.code == C::PriceOutOfRange)
        .expect("код в справочнике");
    assert!(m.no_retry);
    assert_eq!(
        m.inactive.keys().copied().collect::<Vec<_>>(),
        vec!["Retry"]
    );
    // Не безопасный повтор продажи (sell_unsent.rs): без ветки решения 268 — алерт, как прежний OrderRejected.
    let op = arb::core::op_error::OpError::from(&e);
    assert!(!op.retry_safe() && !op.rejected_unsent() && !op.cancel_confirmed());
}

#[test]
fn human_reason_for_known_codes() {
    use arb::notify::reason::human;
    for text in [
        "MEXC, код 30087: Order price exceeds allowed range",
        "BingX, код 100440: Order price deviates too much from market price",
        "Bybit, код 170194: Sell order price cannot be lower than 0.1.",
        "XT, код ORDER_F0501: []",
        "OKX, код 51138: The lowest price limit for sell orders is 1.",
        "HTX, код order-limitorder-price-min-error: Sell price cannot be lower than specific price",
        "CoinEx, код 3606: x",
        "Bitget, код 43008 (HTTP 400): x",
        "Poloniex, код 21311: Order price must be greater than min price (HTTP 400)",
    ] {
        let r = human(text);
        assert!(
            r.text.contains("вне диапазона, разрешённого биржей"),
            "{text}: {}",
            r.text
        );
        assert_eq!(r.answer, None, "{text}");
    }
    // Коды, которые не сопоставлены, словарь не знает.
    for (v, c) in [
        ("mexc", "30018"),
        ("xt", "ORDER_F0601"),
        ("kucoin", "400100"),
    ] {
        assert_eq!(arb::notify::reason_codes_range::lookup(v, c), None);
    }
}
