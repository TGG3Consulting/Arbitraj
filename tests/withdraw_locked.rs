//! Файл: tests/withdraw_locked.rs
//! Что делает: решение 166 (Тигран, 05.11.2026) на живых текстах отказов вывода и примерах официальной документации.
//! (1) Адаптеры переводят «средства временно заблокированы для вывода» в единый код WithdrawLocked: MEXC 16021 (живой
//!     ответ 04.11, сделка 20261104173851521BABYDOGE), KuCoin 115008 (живой ответ 05.11, сделка 20261105020010981ION),
//!     Binance −4060 и Bybit 131085 (официальные таблицы кодов); тот же код не у вывода — прежний смысл.
//! (2) Повтор вывода (core/transfer_locked.rs) на настоящей SQLite во временной папке (ядро без площадок в реестре):
//!     N повторов с интервалом transfer.locked_retry_ms, строка «Вывод: попытка N из M — причина» в хронологии, успех
//!     на k-й попытке, исчерпание, без настроек — без повтора.
//! (3) Разбор «можно вывести» по примерам документации: KuCoin quotas (пример ответа спецификации
//!     kucoin-universal-sdk, x-response-example «Get Withdrawal Quotas»), Bybit withdrawable-amount (пример ответа
//!     «Get Withdrawable Amount»), Gate deposits (поля модели DepositRecord SDK; JSON-примера в документации нет).
//!     Живых приватных записей ещё нет — СВЕРИТЬ на живом ответе (tests/README.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::venues (mexc, kucoin, binance, bybit, gate), arb::settings. Моков нет.

mod usdt_return_common;

use std::time::{Duration, Instant};

use arb::core::precheck_withdraw::verdict;
use arb::core::transfer_locked::{Plan, again, is_locked, next};
use arb::core::types::{VenueId, WithdrawHold};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::settings::value::Scope;
use rust_decimal::Decimal;
use serde_json::json;
use usdt_return_common::{dir, doge, entries, start};

/// Живой текст KuCoin 05.11 (сделка 20261105020010981ION).
const KUCOIN_115008: &str = "Insufficient withdrawable assets: Assets equivalent to 44.91 USDT (898299.261 ION) are \
currently locked from withdrawal. Wait for them to be unlocked or transfer more assets…";
/// Живой текст MEXC 04.11 (сделка 20261104173851521BABYDOGE).
const MEXC_16021: &str = "Some pre-crediting assets are temporarily unavailable for withdrawal. Please reconfirm the quantity.";

/// Отказ KuCoin 115008 через настоящий адаптер. Ошибок нет.
fn kucoin_locked() -> GatewayError {
    use arb::venues::kucoin::errors::{Op, from_code};
    let raw = json!({"code": "115008", "msg": KUCOIN_115008}).to_string();
    from_code("115008", KUCOIN_115008, Op::Withdraw, &raw)
}

#[test]
fn adapters_classify_withdraw_locked() {
    let k = kucoin_locked();
    assert_eq!(k.code, ErrorCode::WithdrawLocked);
    assert!(
        k.message
            .starts_with("KuCoin, код 115008: Insufficient withdrawable"),
        "{}",
        k.message
    );
    assert!(is_locked(&k), "вывод не создан — повтор безопасен");
    let m = {
        use arb::venues::mexc::errors::{Op, from_code};
        let raw = json!({"code": 16021, "msg": MEXC_16021}).to_string();
        from_code(16021, MEXC_16021, Op::Withdraw, &raw)
    };
    assert_eq!(m.code, ErrorCode::WithdrawLocked);
    let b = {
        use arb::venues::binance::errors::{Op, from_code};
        let msg = "As your deposit has not reached the required block confirmations, we have temporarily locked 45 asset";
        from_code(-4060, msg, Op::Withdraw, "{}")
    };
    assert_eq!(b.code, ErrorCode::WithdrawLocked);
    let y = {
        use arb::venues::bybit::errors::{Op, from_code};
        let msg = "Withdrawal amount is greater than your availale balance (the deplayed withdrawal is triggered)";
        from_code(131085, msg, Op::Withdraw, "{}")
    };
    assert_eq!(y.code, ErrorCode::WithdrawLocked);
    // Не вывод — прежний смысл (не «заблокировано для вывода»).
    let other = {
        use arb::venues::kucoin::errors::{Op, from_code};
        from_code("115008", KUCOIN_115008, Op::Other, "{}")
    };
    assert_ne!(other.code, ErrorCode::WithdrawLocked);
    // Человеческая причина (решение 164) — понятная, не «код как есть».
    let r = arb::notify::reason::human(&k.message);
    assert!(r.text.contains("заблокировал средства для вывода"), "{r:?}");
}

#[test]
fn plan_counts_repeats() {
    let p = Some(Plan {
        every: Duration::from_millis(1000),
        max: 10,
    });
    for done in 0..10 {
        assert_eq!(next(p, done), Some(Duration::from_millis(1000)));
    }
    assert_eq!(next(p, 10), None, "10 повторов — больше нет");
    assert_eq!(next(None, 0), None, "без настроек — без повтора");
}

/// Ядро с повторами у kucoin: (мс, раз). Сделка ION сохранена. Ошибок нет.
async fn core_with(
    name: &str,
    retry: Option<(u64, u64)>,
) -> (arb::core::Core, arb::storage::journal::Journal) {
    let (core, j) = start(&dir(name), None).await;
    if let Some((ms, n)) = retry {
        let s = &core.inner.deps.settings;
        let sc = || Scope::Venue("kucoin".into());
        s.set("transfer.locked_retry_ms", sc(), Some(json!(ms)), "тест")
            .await
            .unwrap();
        s.set("transfer.locked_retries_max", sc(), Some(json!(n)), "тест")
            .await
            .unwrap();
    }
    let x = doge("20261105020010981ION");
    arb::core::ledger::save(&core, &x).await;
    (core, j)
}

/// Строки хронологии сделки. Ошибок нет.
fn timeline(core: &arb::core::Core, id: &str) -> Vec<String> {
    let d = core.deals().into_iter().find(|d| d.id == id).unwrap();
    d.timeline.into_iter().map(|(_, t)| t).collect()
}

#[tokio::test]
async fn success_on_kth_attempt() {
    let id = "20261105020010981ION";
    let (core, j) = core_with("ok", Some((50, 10))).await;
    let from = VenueId("kucoin".into());
    let e = kucoin_locked();
    let mut done = 0;
    let t = Instant::now();
    // Первая попытка и два повтора отклонены 115008, третий повтор прошёл (дальше вызывающий не зовёт again).
    for _ in 0..3 {
        assert!(again(&core, id, &from, &e, &mut done).await);
    }
    assert_eq!(done, 3);
    assert!(
        t.elapsed() >= Duration::from_millis(150),
        "интервал выдержан"
    );
    let tl = timeline(&core, id);
    assert!(
        tl.iter()
            .any(|t| t.contains("повтор каждые 50 мс, не больше 10 раз")),
        "{tl:?}"
    );
    assert!(
        tl.iter()
            .any(|t| t.contains("Вывод: попытка 2 из 10 — KuCoin, код 115008")),
        "{tl:?}"
    );
    assert!(
        !tl.iter().any(|t| t.contains("попытка 3 из 10")),
        "успешная попытка — без строки отказа"
    );
    assert_eq!(entries(&j, "core.transfer_locked", id).await.len(), 3);
}

#[tokio::test]
async fn exhausted_after_max_repeats() {
    let id = "20261105020010981ION";
    let (core, _) = core_with("max", Some((10, 3))).await;
    let from = VenueId("kucoin".into());
    let e = kucoin_locked();
    let mut done = 0;
    let mut calls = 0;
    while again(&core, id, &from, &e, &mut done).await {
        calls += 1;
    }
    assert_eq!((calls, done), (3, 3));
    let tl = timeline(&core, id);
    assert!(
        tl.iter()
            .any(|t| t.contains("Вывод: попытка 3 из 3 — KuCoin, код 115008")),
        "{tl:?}"
    );
    assert!(
        tl.iter().any(|t| t.contains(
            "kucoin временно заблокировала средства для вывода — все 3 повторов неудачны"
        )),
        "{tl:?}"
    );
}

#[tokio::test]
async fn without_settings_no_retry() {
    let id = "20261105020010981ION";
    let (core, _) = core_with("noset", None).await;
    let mut done = 0;
    let from = VenueId("kucoin".into());
    assert!(!again(&core, id, &from, &kucoin_locked(), &mut done).await);
    let tl = timeline(&core, id);
    assert!(
        tl.iter().any(|t| t.contains("повтор не настроен")),
        "{tl:?}"
    );
    // Другой отказ вывода — не повтор и без строки.
    let plain = GatewayError::new(ErrorCode::WithdrawRejected, None, "адрес не в белом списке");
    assert!(!again(&core, id, &from, &plain, &mut done).await);
}

#[test]
fn kucoin_quotas_doc_example() {
    // x-response-example спецификации «Get Withdrawal Quotas» (data): lockedAmount "0" — удержаний нет.
    let ex = json!({"currency":"BTC","limitBTCAmount":"15.79590095","usedBTCAmount":"0.00000000","quotaCurrency":"USDT",
        "limitQuotaCurrencyAmount":"999999.00000000","usedQuotaCurrencyAmount":"0","remainAmount":"15.79590095",
        "availableAmount":"0","withdrawMinFee":"0.0005","innerWithdrawMinFee":"0","withdrawMinSize":"0.001",
        "isWithdrawEnabled":true,"precision":8,"chain":"BTC","reason":null,"lockedAmount":"0"});
    let h = arb::venues::kucoin::withdraw_quota::parse(&ex).unwrap();
    assert!(h.is_empty() && !h.is_unknown());
    // Тот же формат по USDT с удержанием живого случая 05.11 (44.91 USDT): покупка 45 при свободных 50 — отказ.
    let mut usdt = ex.clone();
    usdt["currency"] = json!("USDT");
    usdt["lockedAmount"] = json!("44.91");
    let h = arb::venues::kucoin::withdraw_quota::parse(&usdt).unwrap();
    assert_eq!(h.items.len(), 1);
    assert_eq!(
        (h.items[0].asset.as_str(), h.items[0].amount),
        ("USDT", Decimal::new(4491, 2))
    );
    let e = verdict(
        "kucoin",
        "USDT",
        Decimal::from(50),
        h.items[0].amount,
        Decimal::from(45),
        &h.items[0].reason,
    )
    .unwrap_err();
    assert!(e.contains("KuCoin держит от вывода 44.91 USDT"), "{e}");
    assert!(e.contains("вывод не пройдёт"), "{e}");
}

#[test]
fn bybit_withdrawable_doc_example() {
    // Пример ответа «Get Withdrawable Amount» (result): доступно = можно вывести — удержаний нет.
    let ex = json!({"limitAmountUsd":"595051.7","withdrawableAmount":{
        "FUND":{"coin":"USDT","withdrawableAmount":"155805.847","availableBalance":"155805.847"},
        "UTA":{"coin":"USDT","withdrawableAmount":"498751.0882","availableBalance":"498751.0882"}}});
    let h = arb::venues::bybit::withdrawable::parse(&ex, "UTA").unwrap();
    assert!(h.is_empty());
    let mut held = ex.clone();
    held["withdrawableAmount"]["UTA"]["withdrawableAmount"] = json!("498700");
    let h = arb::venues::bybit::withdrawable::parse(&held, "FUND,UTA").unwrap();
    assert_eq!(h.items.len(), 1);
    assert_eq!(h.items[0].amount, "51.0882".parse::<Decimal>().unwrap());
    // Счёт не из venue.withdraw_account — не считается.
    assert!(
        arb::venues::bybit::withdrawable::parse(&held, "FUND")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn gate_dep_credited_is_hold() {
    // Поля модели DepositRecord (SDK gateapi-python); живой записи нет — сверить на живом ответе.
    let list = json!([
        {"id":"1","txid":"0xa","currency":"USDT","amount":"45","chain":"BSC","status":"DEP_CREDITED"},
        {"id":"2","txid":"0xb","currency":"USDT","amount":"100","chain":"BSC","status":"DONE"}
    ]);
    let h = arb::venues::gate::deposit_hold::parse(&list).unwrap();
    assert_eq!(h.items.len(), 1);
    assert_eq!(h.items[0].amount, Decimal::from(45));
}

#[test]
fn unknown_is_not_empty_answer() {
    let u = WithdrawHold::unknown("нет в документации");
    assert!(u.is_unknown() && u.is_empty());
    assert!(!WithdrawHold::of(vec![]).is_unknown());
}
