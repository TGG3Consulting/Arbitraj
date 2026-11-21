//! Файл: tests/gate_deposit_plan.rs
//! Что делает: решение 387 — проверки фонового круга сведений депозита Gate и запроса минимума по требованию ядра:
//! план круга (deposit_plan.rs) на реально записанном справочнике монет Gate (tests/data/cex_samples_2026_11_04/
//! gate_spot_currencies.json, GET /spot/currencies), классификация сбоев (в том числе живой ответ «HTTP 400,
//! INVALID_PARAM_VALUE: deposit disabled» из журнала бота 17.11), перенос сведений монеты в записи справочника
//! (deposit_min.rs apply / stale), правило повтора запроса ядра (core/deposit_min_ask.rs due). Сети нет.
//! Кто вызывает: cargo test.
//! От чего зависит: venues/gate (deposit_plan, deposit_min, networks_parse, errors), core/deposit_min_ask.rs.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::deposit_min_ask::{Outcome, State, due, outcome};
use arb::core::types::{Asset, CoinNetworkInfo, VenueId};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::gate::deposit_min::{apply, stale};
use arb::venues::gate::deposit_plan::{combine, need_fetch, plan, stops_round, transient};
use arb::venues::gate::errors::{Op, from_label};
use arb::venues::gate::networks_parse::{self, Extra};

const DIR: &str = "tests/data/cex_samples_2026_11_04";

/// Записанный справочник монет Gate.
fn currencies() -> Value {
    let raw = std::fs::read_to_string(format!("{DIR}/gate_spot_currencies.json")).expect("запись");
    serde_json::from_str(&raw).expect("JSON")
}

/// Справочник сетей формата ядра по записи (сеть BSC — «BSC», как в записи; withdraw_status — пустой).
fn networks() -> Vec<CoinNetworkInfo> {
    networks_parse::parse_networks(
        &currencies(),
        &Value::Array(vec![]),
        "BSC",
        &VenueId("gate".into()),
        false,
        &HashMap::new(),
        &|_| Some(2),
    )
    .expect("разбор")
    .list
}

/// Монеты подписки, как их собирает ядро (core/subscribe.rs wanted): сеть BSC с адресом контракта.
fn subscription(list: &[CoinNetworkInfo]) -> Vec<Asset> {
    list.iter()
        .filter(|n| n.network == "BSC" && n.asset.contract.is_some())
        .map(|n| n.asset.clone())
        .collect()
}

#[test]
fn plan_skips_closed_deposit_on_recording() {
    let list = networks();
    let assets = subscription(&list);
    // Ожидание — прямо из записи: ввод в BSC закрыт (deposit_disabled сети BSC) или монета снята (delisted).
    let mut closed_raw: HashSet<String> = HashSet::new();
    for c in currencies().as_array().expect("массив") {
        let delisted = c["delisted"] == Value::Bool(true);
        for ch in c["chains"].as_array().into_iter().flatten() {
            if ch["name"] == "BSC" && (delisted || ch["deposit_disabled"] == Value::Bool(true)) {
                closed_raw.insert(c["currency"].as_str().expect("currency").to_string());
            }
        }
    }
    let expect_closed = assets
        .iter()
        .filter(|a| closed_raw.contains(&a.ticker))
        .count();
    let p = plan(
        &assets,
        Some(list.as_slice()),
        "BSC",
        &|_: &str| false,
        None,
    );
    assert!(
        expect_closed > 0,
        "в записи есть монеты с закрытым вводом в BSC"
    );
    assert_eq!(p.closed, expect_closed);
    assert_eq!(p.no_bsc, 0);
    assert_eq!(p.ask.len() + p.closed, assets.len());
    assert!(p.ask.iter().all(|a| !closed_raw.contains(&a.ticker)));
    // CAKE — ввод в BSC открыт (tests/gate_real_data.rs) — спрашивается.
    assert!(p.ask.iter().any(|a| a.ticker == "CAKE"));
}

#[test]
fn plan_orders_unknown_first_and_filters_retry() {
    let list = networks();
    let assets = subscription(&list);
    let all = plan(
        &assets,
        Some(list.as_slice()),
        "BSC",
        &|_: &str| false,
        None,
    );
    assert!(all.ask.len() > 2);
    let first = all.ask[0].ticker.clone();
    // Минимум первой монеты уже известен — она уходит в конец круга.
    let p = plan(
        &assets,
        Some(list.as_slice()),
        "BSC",
        &|t: &str| t == first,
        None,
    );
    assert_eq!(
        p.ask.last().map(|a| a.ticker.as_str()),
        Some(first.as_str())
    );
    assert_eq!(p.ask.len(), all.ask.len());
    // Повтор — только монеты со сбоем.
    let only: HashSet<String> = ["CAKE".to_string()].into();
    let r = plan(
        &assets,
        Some(list.as_slice()),
        "BSC",
        &|_: &str| false,
        Some(&only),
    );
    assert_eq!(r.ask.len(), 1);
    assert_eq!(r.ask[0].ticker, "CAKE");
    // Справочника ещё нет — спрашиваются все.
    let none = plan(&assets, None, "BSC", &|_: &str| false, None);
    assert_eq!(none.ask.len(), assets.len());
    assert_eq!(none.closed, 0);
    // Монеты без сети BSC в справочнике (другое название сети) не спрашиваются.
    let other = plan(
        &assets,
        Some(list.as_slice()),
        "NO_SUCH_CHAIN",
        &|_: &str| false,
        None,
    );
    assert_eq!(other.no_bsc, assets.len());
}

#[test]
fn failure_classes() {
    // Живой ответ из журнала бота 17.11: «(Gate, HTTP 400, INVALID_PARAM_VALUE: deposit disabled)».
    let e = from_label(
        400,
        "INVALID_PARAM_VALUE",
        "deposit disabled",
        Op::Other,
        "{}",
    );
    assert_eq!(
        e.message,
        "Gate, HTTP 400, INVALID_PARAM_VALUE: deposit disabled"
    );
    assert!(
        !transient(e.code),
        "«deposit disabled» не повторяется до полного круга"
    );
    assert!(transient(ErrorCode::SecretsLocked));
    assert!(transient(ErrorCode::RateLimited));
    assert!(transient(ErrorCode::Unreachable));
    assert!(transient(ErrorCode::Timeout));
    assert!(!transient(ErrorCode::InvalidAddress));
    assert!(stops_round(ErrorCode::SecretsLocked));
    assert!(!stops_round(ErrorCode::RateLimited));
}

#[test]
fn apply_moves_min_and_confirms() {
    let mut list = networks();
    let e = Extra {
        deposit_min: Some(Decimal::new(5, 1)),
        confirms: [("BSC".to_string(), 15u32)].into(),
        ..Default::default()
    };
    assert!(stale(&list, "CAKE", &e));
    apply(&mut list, "CAKE", &e);
    assert!(!stale(&list, "CAKE", &e));
    let cake = list
        .iter()
        .find(|n| n.asset.ticker == "CAKE" && n.network == "BSC")
        .expect("CAKE");
    assert_eq!(cake.deposit_min, Some(Decimal::new(5, 1)));
    assert_eq!(cake.deposit_confirmations, Some(15));
    // Другие монеты не тронуты.
    let usdt = list
        .iter()
        .find(|n| n.asset.ticker == "USDT" && n.network == "BSC")
        .expect("USDT");
    assert_eq!(usdt.deposit_min, None);
    // Неизвестный минимум не затирает известный.
    let empty = Extra::default();
    assert!(!stale(&list, "CAKE", &empty));
}

#[test]
fn core_ask_retry_rule() {
    let now = Instant::now();
    let every = Some(Duration::from_secs(60));
    assert!(due(None, now, every));
    assert!(!due(Some(State::Busy), now, every));
    assert!(!due(Some(State::Answered), now, every));
    assert!(!due(Some(State::Failed(now)), now, every));
    let later = now + Duration::from_secs(61);
    assert!(due(Some(State::Failed(now)), later, every));
    // Интервал опроса не задан — повтор только после смены секретов (forget).
    assert!(!due(Some(State::Failed(now)), later, None));
}

#[test]
fn address_only_when_minimum_needed() {
    // Минимум не известен, сеть задана, ввод по справочнику открыт или справочника нет — запрашивать.
    assert!(need_fetch(false, "BSC", Some(true), false));
    assert!(need_fetch(false, "BSC", None, false));
    // Минимум уже есть — нет.
    assert!(!need_fetch(true, "BSC", Some(true), false));
    // Ввод в BSC закрыт по справочнику (монеты, которые с Gate только выводят; 724 из 1075 на записи 04.11) — нет.
    assert!(!need_fetch(false, "BSC", Some(false), false));
    // «deposit disabled» уже получен при этом справочнике — не повторяем.
    assert!(!need_fetch(false, "BSC", Some(true), true));
    // Подсказка имён сети: venue.bsc_network не задана (пусто) — нет (ревью Н1).
    assert!(!need_fetch(false, "", None, false));
    assert!(!need_fetch(false, "  ", Some(true), false));
}

#[test]
fn address_failure_keeps_networks() {
    let list = networks()
        .into_iter()
        .filter(|n| n.asset.ticker == "CAKE")
        .collect::<Vec<_>>();
    let n = list.len();
    let fail = GatewayError::new(ErrorCode::Timeout, None, "срок");
    let (got, err) = combine(Err(fail), Ok(list.clone()));
    assert_eq!(got.expect("сети монеты есть").len(), n);
    assert_eq!(err.map(|e| e.code), Some(ErrorCode::Timeout));
    let disabled = from_label(
        400,
        "INVALID_PARAM_VALUE",
        "deposit disabled",
        Op::Other,
        "{}",
    );
    let (got, err) = combine(Err(disabled), Ok(list.clone()));
    assert!(got.is_ok());
    assert!(err.is_some());
    let (got, err) = combine(Ok(()), Ok(list));
    assert!(got.is_ok() && err.is_none());
}

#[test]
fn core_ask_outcomes() {
    assert_eq!(outcome(Ok(true)), Outcome::Answered);
    // Ответ без минимума («временно пусто», obtain_failed, ввод отклонён) — повтор по интервалу, а не «никогда».
    assert_eq!(outcome(Ok(false)), Outcome::Retry);
    assert_eq!(outcome(Err(ErrorCode::Timeout)), Outcome::Retry);
    assert_eq!(outcome(Err(ErrorCode::SecretsLocked)), Outcome::Retry);
    // Минимума отдельным запросом площадка не даёт (метод контракта по умолчанию) — больше не спрашивать.
    assert_eq!(outcome(Err(ErrorCode::NotApplicable)), Outcome::Unsupported);
}
