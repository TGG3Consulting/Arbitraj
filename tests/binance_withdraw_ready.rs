//! Файл: tests/binance_withdraw_ready.rs
//! Что проверяет: решение 253 (Binance: вывод сразу после покупки получал отказ) на примерах официальной документации
//! Binance (tests/data/binance_docs_2026-11-10, README там же), текстах отказов из сделок бота 10.11 и настоящем ядре
//! (SQLite во временной папке, без площадок в реестре):
//! (а) поток аккаунта: outboundAccountPosition → остатки (монета, свободно); ожидание доступности — сразу, если
//!     свободно ≥ суммы; ждёт событие и выходит по нему; вывод с funding — не сверяется; строки хронологии;
//! (б) −4026 / −4024 вывода в окне venue.withdraw_unready_window_ms после своей покупки → WithdrawLocked (повтор
//!     core/transfer_locked.rs, как −4060); вне окна, без окна, другой код — как раньше (WithdrawRejected);
//! (г) окончательный отказ вывода закрывает входы монеты с этой биржи (coin.entry_blocked_venues), другие биржи — нет;
//! (д) фактическая комиссия — transactionFee истории выводов; поправка затрат по фактам прошлого перевода.
//! Дополнение t89 (возврат USDT после продажи) — tests/binance_withdraw_after_sale.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::binance, arb::core (withdraw_wait, withdraw_block, withdraw_seen, transfer_locked).
//! Моков нет.

mod usdt_return_common;

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::op_error::OpError;
use arb::core::transfer_locked::{again, is_locked};
use arb::core::types::{Asset, Cost, CostKind, Direction, TransferStatus, VenueId, WithdrawReady};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::settings::value::Scope;
use arb::venues::binance::account_update::{Event, parse};
use arb::venues::binance::errors::{Op, from_code};
use arb::venues::binance::{transfers, withdraw_ready};
use usdt_return_common::{dir, doge, start};

const DIR: &str = "tests/data/binance_docs_2026-11-10";

fn venue() -> VenueId {
    VenueId("binance".into())
}

/// Окружение адаптера Binance на настройках ядра. Ошибок нет.
fn bn(core: &arb::core::Core, d: &std::path::Path) -> arb::venues::binance::ctx::Ctx {
    let s = core.inner.deps.settings.clone();
    arb::venues::binance::ctx::Ctx {
        settings: s.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal: core.inner.deps.journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.join("raw"), s),
        taps: Default::default(),
    }
}

async fn set(core: &arb::core::Core, name: &str, v: Value) {
    core.inner
        .deps
        .settings
        .set(name, Scope::Venue("binance".into()), Some(v), "тест")
        .await
        .unwrap();
}

/// Отказ вывода Binance кодом `code` с текстом из сделки бота (через настоящий разбор адаптера). Ошибок нет.
fn refusal(code: i64, msg: &str) -> GatewayError {
    let raw = json!({"code": code, "msg": msg}).to_string();
    from_code(code, msg, Op::Withdraw, &raw)
}

#[test]
fn account_position_doc_example() {
    let t = std::fs::read_to_string(format!("{DIR}/outbound_account_position.json")).unwrap();
    match parse(&t, &venue()) {
        Event::Position(list, at) => {
            assert_eq!(list, vec![("ETH".to_string(), Decimal::new(10_000, 0))]);
            assert_eq!(at, 1_564_034_571_105);
        }
        other => panic!("ожидался Position: {other:?}"),
    }
}

#[tokio::test]
async fn wait_returns_on_event_or_at_once() {
    let d = dir("bn251wait");
    let (core, _) = start(&d, None).await;
    let ctx = bn(&core, &d);
    set(&core, "venue.withdraw_wallet", json!("spot")).await;
    let coin = |t: &str| Asset {
        ticker: t.into(),
        bsc_contract: None,
    };
    let amount = Decimal::new(1234, 1);
    // Остаток уже есть — сразу.
    withdraw_ready::on_position(&[("W251A".into(), amount)], 1);
    let r = withdraw_ready::wait(&ctx, &coin("W251A"), amount, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(r.ready, Some(true));
    assert!(r.source.contains("outboundAccountPosition"), "{}", r.source);
    // Остатка нет — ждём событие (через 100 мс), выход по нему, не по сроку.
    withdraw_ready::on_position(&[("W251B".into(), Decimal::ONE)], 1);
    let t = Instant::now();
    let w = tokio::spawn({
        let ctx = ctx.clone();
        async move { withdraw_ready::wait(&ctx, &coin("W251B"), amount, Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Старое событие (раньше записанного) не перезаписывает остаток.
    withdraw_ready::on_position(&[("W251B".into(), amount)], 0);
    assert_eq!(withdraw_ready::free("W251B"), Some(Decimal::ONE));
    withdraw_ready::on_position(&[("W251B".into(), amount + Decimal::ONE)], 2);
    let r = w.await.unwrap().unwrap();
    assert_eq!(r.ready, Some(true));
    assert_eq!(r.free, Some(amount + Decimal::ONE));
    assert!(t.elapsed() < Duration::from_secs(4), "вышли по событию");
    // Вывод с funding — остатки спота не сверяются.
    set(&core, "venue.withdraw_wallet", json!("funding")).await;
    let r = withdraw_ready::wait(&ctx, &coin("W251C"), amount, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(r.ready, None);
}

#[test]
fn chronology_lines() {
    use arb::core::withdraw_wait::line;
    let q = Decimal::new(1_234_567, 3);
    let ok = WithdrawReady {
        ready: Some(true),
        free: Some(Decimal::new(1_300_000, 3)),
        source: "событие баланса outboundAccountPosition".into(),
    };
    assert_eq!(
        line(412, "CYBER", q, Ok(ok)),
        "Ожидание доступности монеты для вывода: 412 мс — свободно 1300 ≥ 1234.567 CYBER (событие баланса outboundAccountPosition)"
    );
    let low = WithdrawReady {
        ready: Some(false),
        free: Some(Decimal::ZERO),
        source: "REST".into(),
    };
    assert!(line(3000, "CYBER", q, Ok(low)).contains("< 1234.567 CYBER (REST); вывод отправлен"));
    assert!(
        line(0, "X", q, Ok(WithdrawReady::default()))
            .contains("площадка доступность вывода не сообщает")
    );
    assert!(
        line(5, "X", q, Err("нет связи".into()))
            .contains("проверка не удалась (нет связи), вывод сразу")
    );
}

#[tokio::test]
async fn not_ready_codes_in_window_become_locked() {
    let d = dir("bn251codes");
    let (core, _) = start(&d, None).await;
    let ctx = bn(&core, &d);
    let e26 = refusal(-4026, "[031033] User has insufficient balance");
    let e24 = refusal(-4024, "[031031] User does not own this currency");
    assert_eq!(
        e26.code,
        ErrorCode::WithdrawRejected,
        "без окна — как раньше"
    );
    // Окно не задано — как раньше, даже сразу после покупки.
    withdraw_ready::traded("CYBER251", tokio::time::Instant::now());
    assert_eq!(
        transfers::not_ready(&ctx, "CYBER251", e26.clone()).code,
        ErrorCode::WithdrawRejected
    );
    set(&core, "venue.withdraw_unready_window_ms", json!(5000)).await;
    for e in [&e26, &e24] {
        let x = transfers::not_ready(&ctx, "CYBER251", e.clone());
        assert_eq!(x.code, ErrorCode::WithdrawLocked, "{}", x.message);
        assert!(
            x.message.contains("ещё не доступна для вывода"),
            "{}",
            x.message
        );
        assert!(is_locked(&x), "вывод не создан — повтор безопасен");
    }
    // Своей покупки монеты не было — как раньше.
    assert_eq!(
        transfers::not_ready(&ctx, "WLFI251", e26.clone()).code,
        ErrorCode::WithdrawRejected
    );
    // Другой отказ вывода (адрес не в белом списке −4035 — отказ аккаунта, InvalidAddress) — не «ещё не доступна».
    let wl = refusal(
        -4035,
        "This address is not on the whitelist. Please join and try again.",
    );
    assert_eq!(wl.code, ErrorCode::InvalidAddress);
    assert_eq!(
        transfers::not_ready(&ctx, "CYBER251", wl).code,
        ErrorCode::InvalidAddress
    );
    // Вне окна — как раньше.
    set(&core, "venue.withdraw_unready_window_ms", json!(1)).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        transfers::not_ready(&ctx, "CYBER251", e26.clone()).code,
        ErrorCode::WithdrawRejected
    );
    // Повтор ядра — по transfer.locked_* площадки (как −4060).
    set(&core, "transfer.locked_retry_ms", json!(20)).await;
    set(&core, "transfer.locked_retries_max", json!(2)).await;
    set(&core, "venue.withdraw_unready_window_ms", json!(60_000)).await;
    let id = "20261110120000000CYBER";
    arb::core::ledger::save(&core, &doge(id)).await;
    let x = transfers::not_ready(&ctx, "CYBER251", e26);
    let mut done = 0;
    while again(&core, id, &venue(), &x, &mut done).await {}
    assert_eq!(done, 2);
}

#[tokio::test]
async fn refusal_closes_entries_from_that_exchange_only() {
    let d = dir("bn251block");
    let (core, j) = start(&d, None).await;
    let cyber = Asset {
        ticker: "CYBER".into(),
        bsc_contract: Some(alloy_primitives::address!(
            "0x14778860e937f509e651192a90589de711fb88a9"
        )),
    };
    let dir_of = |v: &str| Direction {
        asset: cyber.clone(),
        buy_on: VenueId(v.into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    assert!(arb::core::withdraw_block::check(&core, &dir_of("binance")).is_ok());
    let e = OpError::from(&refusal(-4026, "[031033] User has insufficient balance"));
    arb::core::withdraw_block::close(&core, "20261110120000000CYBER", &cyber, &venue(), &e).await;
    let why = arb::core::withdraw_block::check(&core, &dir_of("binance")).unwrap_err();
    assert!(why.contains("входы с binance закрыты"), "{why}");
    assert!(
        why.contains("[031033] User has insufficient balance"),
        "{why}"
    );
    assert!(
        arb::core::withdraw_block::check(&core, &dir_of("bingx")).is_ok(),
        "другая биржа — открыта"
    );
    // Второй отказ той же биржи — не дублирует её в списке.
    arb::core::withdraw_block::close(&core, "x", &cyber, &venue(), &e).await;
    let scope = arb::core::blacklist::coin_scope(&cyber);
    let id = match &scope {
        Scope::Coin(id) => id.clone(),
        _ => unreachable!(),
    };
    let at = arb::settings::read::At {
        venue: None,
        coin: Some(&id),
    };
    let list = core
        .inner
        .deps
        .settings
        .list(arb::core::withdraw_block::VENUES, at)
        .unwrap();
    assert_eq!(list, vec!["binance".to_string()]);
    // Отказ не из refusal (ордер) — не закрывает.
    let other = OpError::from(&GatewayError::new(ErrorCode::OrderRejected, None, "ордер"));
    arb::core::withdraw_block::close(&core, "y", &cyber, &VenueId("mexc".into()), &other).await;
    assert!(arb::core::withdraw_block::check(&core, &dir_of("mexc")).is_ok());
    assert!(
        !usdt_return_common::entries(&j, "core.withdraw_block", "CYBER")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn history_fee_and_seen_cost() {
    let dp = dir("bn251fee");
    let (core, _) = start(&dp, None).await;
    let ctx = bn(&core, &dp);
    let list: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{DIR}/withdraw_history.json")).unwrap(),
    )
    .unwrap();
    let rec = &list[0];
    let t = transfers::transfer(
        &ctx,
        rec,
        "b6ae22b3aa844210a7041aee7589627c",
        TransferStatus::Sent,
    )
    .unwrap();
    let fee = t.fee.expect("transactionFee");
    assert_eq!(
        (fee.kind, fee.amount, fee.asset.as_str()),
        (CostKind::WithdrawFee, Decimal::new(4, 3), "USDT")
    );
    // В расчёт — только при transfer.seen_cost площадки (одна формула и у оценки «Перегона» — adjust_at).
    let coin = Asset {
        ticker: "SQQQB".into(),
        bsc_contract: None,
    };
    let info = net(Some(Decimal::ZERO), None);
    let s = fact("SQQQB", "1000", "990.25", Some("0"));
    arb::core::withdraw_seen::record(&core, s).await;
    let dir_costs = lines(Decimal::from(500), Decimal::ZERO, Decimal::ZERO);
    let a = Decimal::from(500);
    let off = arb::core::withdraw_seen::adjust_at(
        &core,
        &venue(),
        &coin,
        &info,
        Decimal::ZERO,
        a,
        dir_costs.clone(),
    );
    assert_eq!(off, dir_costs, "transfer.seen_cost не задана — как раньше");
    set(&core, "transfer.seen_cost", json!(true)).await;
    let on = arb::core::withdraw_seen::adjust_at(
        &core,
        &venue(),
        &coin,
        &info,
        Decimal::ZERO,
        a,
        dir_costs,
    );
    assert_eq!(loss(&on), d("4.875"), "0.975 % от 500");
}

fn d(x: &str) -> Decimal {
    x.parse().unwrap()
}

/// Сеть монеты в справочнике: фиксированная комиссия и доля суммы.
fn net(fee: Option<Decimal>, pct: Option<Decimal>) -> arb::core::types::CoinNetworkInfo {
    arb::core::types::CoinNetworkInfo {
        venue: venue(),
        asset: Asset {
            ticker: "X".into(),
            bsc_contract: None,
        },
        network: "BSC".into(),
        deposit_enabled: true,
        withdraw_enabled: true,
        withdraw_fee: fee,
        withdraw_fee_pct: pct,
        withdraw_min: None,
        withdraw_max: None,
        deposit_min: None,
        deposit_confirmations: None,
        withdraw_precision: None,
        withdraw_step: None,
        updated_at: chrono::Utc::now(),
    }
}

/// Сырые факты перевода: отправлено, зачислено, комиссия по факту.
fn fact(t: &str, sent: &str, credited: &str, fee: Option<&str>) -> arb::core::withdraw_seen::Seen {
    arb::core::withdraw_seen::Seen {
        asset: Asset {
            ticker: t.into(),
            bsc_contract: None,
        },
        from: venue(),
        sent: d(sent),
        credited: d(credited),
        fee: fee.map(d),
    }
}

/// Затраты справочника на сумму a (как costs_withdraw.rs): комиссия, налог (a − комиссия) × t %.
fn lines(a: Decimal, fee: Decimal, t: Decimal) -> Vec<Cost> {
    vec![
        Cost::new(CostKind::WithdrawFee, fee, "X", false),
        Cost::new(
            CostKind::TransferTax,
            (a - fee) * t / Decimal::ONE_HUNDRED,
            "X",
            false,
        ),
    ]
}

/// Потеря монеты при переводе по строкам: комиссия + налог + прочее.
fn loss(c: &[Cost]) -> Decimal {
    c.iter()
        .filter(|c| {
            matches!(
                c.kind,
                CostKind::WithdrawFee | CostKind::TransferTax | CostKind::TransferShortfall
            )
        })
        .map(|c| c.amount)
        .sum()
}

/// Строка вида kind (0 — нет). Ошибок нет.
fn of(c: &[Cost], k: CostKind) -> Decimal {
    c.iter().filter(|c| c.kind == k).map(|c| c.amount).sum()
}

/// Решение 253 (д), дополнение (Тигран 10.11): каждая составляющая потери перевода — ровно один раз. Числа SQQQB/SOXSB —
/// из сообщения техлида (недостача ~0.975 % при комиссии 0); CYBER — условные (живого успешного вывода CYBER с Binance
/// нет: все 12 сделок 10.11 — отказ).
#[test]
fn transfer_loss_counted_exactly_once() {
    use arb::core::withdraw_seen::model;
    let z = Decimal::ZERO;
    let s1000 = Decimal::from(1000);
    // SQQQB: API комиссии 0, факт 0, налог не измерен (t = 0), дошло на 0.975 % меньше — всё в «прочее».
    let sq = fact("SQQQB", "1000", "990.25", Some("0"));
    let c = model(&sq, Some(z), z, s1000, lines(s1000, z, z));
    assert_eq!(of(&c, CostKind::TransferShortfall), d("9.75"));
    assert_eq!(loss(&c), d("9.75"), "= отправлено − зачислено");
    // Потом налог измерили (0.975 %): прочее пересчитывается против нового налога — 0, двойного счёта нет.
    let t = d("0.975");
    let c = model(&sq, Some(z), t, s1000, lines(s1000, z, t));
    assert_eq!(of(&c, CostKind::TransferTax), d("9.75"));
    assert_eq!(of(&c, CostKind::TransferShortfall), z);
    assert_eq!(loss(&c), d("9.75"));
    // SOXSB: то же при факте «комиссии нет» (биржа не сообщила) и справочнике 0 — тоже в «прочее».
    let so = fact("SOXSB", "250", "247.5625", None);
    let c = model(
        &so,
        Some(z),
        z,
        Decimal::from(250),
        lines(Decimal::from(250), z, z),
    );
    assert_eq!(loss(&c), d("2.4375"));
    // Комиссия из API = 0, а биржа удержала 2 и сообщила это в истории выводов — в комиссии, не в прочем.
    let held = fact("CYBER", "1000", "998", Some("2"));
    let c = model(&held, Some(z), z, s1000, lines(s1000, z, z));
    assert_eq!(of(&c, CostKind::WithdrawFee), d("2"));
    assert_eq!(of(&c, CostKind::TransferShortfall), z);
    // Удержала 2, но сообщила 0 — в прочем (недоучёта нет).
    let hidden = fact("CYBER", "1000", "998", Some("0"));
    let c = model(&hidden, Some(z), z, s1000, lines(s1000, z, z));
    assert_eq!((of(&c, CostKind::WithdrawFee), loss(&c)), (z, d("2")));
    // CYBER: справочник 1.5, факт 3, налог 1 %, сверх — ещё 0.5 % «прочего». При A = S сумма строк = S − C.
    let t = Decimal::ONE;
    let exp = (s1000 - d("3")) * d("0.99");
    let credited = exp * d("0.995");
    let cy = arb::core::withdraw_seen::Seen {
        credited,
        ..fact("CYBER", "1000", "0", Some("3"))
    };
    let c = model(&cy, Some(d("1.5")), t, s1000, lines(s1000, d("1.5"), t));
    assert_eq!(of(&c, CostKind::WithdrawFee), d("3"));
    assert_eq!(
        of(&c, CostKind::TransferTax),
        d("9.97"),
        "налог — от фактической комиссии"
    );
    assert_eq!(
        loss(&c),
        s1000 - credited,
        "комиссия + налог + прочее = отправлено − зачислено"
    );
    // На другой сумме — фиксированная часть комиссии по факту, налог и прочее долей от ушедшего в сеть.
    let a = Decimal::from(400);
    let c = model(&cy, Some(d("1.5")), t, a, lines(a, d("1.5"), t));
    assert_eq!(of(&c, CostKind::WithdrawFee), d("3"));
    assert_eq!(
        of(&c, CostKind::TransferShortfall),
        (a - d("3")) * d("0.99") * d("0.005")
    );
    // Комиссия долей суммы (0.1 % в справочнике): отличие факта от справочника на S добавляется к справочнику на A.
    let info = net(Some(z), Some(d("0.001")));
    let pr = fact("CYBER", "1000", "998", Some("2"));
    let fee_a = info.fee_for(a).unwrap();
    let c = model(&pr, info.fee_for(s1000), z, a, lines(a, fee_a, z));
    assert_eq!(
        of(&c, CostKind::WithdrawFee),
        d("1.4"),
        "0.4 по справочнику + (2 − 1) по факту"
    );
    // Пришло больше ожидаемого — прочего нет (налог и комиссия из API/пробы не уменьшаются).
    let more = fact("CYBER", "1000", "1000", Some("0"));
    let c = model(&more, Some(z), t, s1000, lines(s1000, z, t));
    assert_eq!(
        (
            of(&c, CostKind::TransferShortfall),
            of(&c, CostKind::TransferTax)
        ),
        (z, d("10"))
    );
}

#[test]
fn min_delay_after_buy() {
    use arb::core::withdraw_wait::left_ms;
    let t = chrono::Utc::now();
    assert_eq!(
        left_ms(t, t + chrono::Duration::milliseconds(300), 1000),
        Some(Duration::from_millis(700))
    );
    assert_eq!(
        left_ms(t, t + chrono::Duration::milliseconds(1500), 1000),
        None,
        "пауза уже прошла"
    );
}

/// Дополнение (Тигран 10.11): отказы аккаунта по официальной таблице Binance (−4007, −4012, −4033…−4036) —
/// InvalidAddress; монета не закрывается, уходит алерт «проблема в аккаунте» с человеческой причиной.
#[tokio::test]
async fn account_refusal_alerts_without_closing_coin() {
    for code in [-4007, -4012, -4033, -4034, -4035, -4036] {
        assert_eq!(refusal(code, "x").code, ErrorCode::InvalidAddress, "{code}");
        // Не вывод — прежний смысл.
        assert_ne!(
            from_code(code, "x", Op::Other, "{}").code,
            ErrorCode::InvalidAddress
        );
    }
    let r = arb::notify::reason::human(
        &refusal(-4035, "This address is not on the whitelist.").message,
    );
    assert!(
        r.text.contains("нет в белом списке вывода Binance"),
        "{r:?}"
    );
    let d = dir("bn253acc");
    let (core, j) = start(&d, None).await;
    let coin = Asset {
        ticker: "SQQQB".into(),
        bsc_contract: Some(alloy_primitives::address!(
            "0x00000000000000000000000000000000000253a1"
        )),
    };
    let e = OpError::from(&refusal(-4035, "This address is not on the whitelist."));
    arb::core::withdraw_block::close(&core, "20261110130000000SQQQB", &coin, &venue(), &e).await;
    let dir_b = Direction {
        asset: coin.clone(),
        buy_on: venue(),
        sell_on: VenueId("bsc_dex".into()),
    };
    assert!(
        arb::core::withdraw_block::check(&core, &dir_b).is_ok(),
        "монета не закрыта"
    );
    let logs =
        usdt_return_common::entries(&j, "core.withdraw_block", "проблема в аккаунте binance").await;
    assert_eq!(logs.len(), 1);
}
