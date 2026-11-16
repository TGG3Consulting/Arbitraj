//! Файл: tests/deposit_min_zero_t163.rs
//! Что делает: решения 369, 370 (Тигран 17.11, задача t163) — общий разбор минимума депозита сети
//! (gateway/deposit_min.rs) у XT, Bitget, HTX, Phemex, CoinEx: «0» — минимум 0, нет поля / null — по
//! venue.deposit_min_absent_ok, нечитаемое (не число, меньше нуля) — неизвестен при любой настройке.
//! Записанных ответов справочника сетей Bitget, HTX, Phemex, CoinEx в репозитории нет (tests/*_real_data.rs — #[ignore]
//! до записи), поэтому значения поля — из ПРИМЕРОВ официальной документации (по одному полю, как на странице), а разбор
//! числа — функцией dec_opt самого адаптера: Bitget Get Coin Info `"minDepositAmount": "0.001"`, HTX APIv2 Currency &
//! Chains `"minDepositAmt":"0.0001"`, Phemex Query deposit chain settings `"minAmountRv": "0"`, CoinEx Get Deposit and
//! Withdrawal Configuration `"min_deposit_amount": "0.023"`. XT — на записи бота: tests/xt_deposit_min_t163.rs.
//! Кто вызывает: cargo test. От чего зависит: arb::gateway::deposit_min, arb::venues::{bitget,htx,phemex,coinex}::num.
//! Моков нет.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::gateway::deposit_min::{Raw, classify, note, resolve};

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Разбор поля так же, как адаптер (его dec_opt), и итог при обоих значениях настройки.
fn both(raw: Raw) -> (Option<Decimal>, Option<Decimal>) {
    (resolve(&raw, false), resolve(&raw, true))
}

#[test]
fn doc_examples_of_four_exchanges() {
    let bitget = json!({"minDepositAmount": "0.001"});
    let htx = json!({"minDepositAmt": "0.0001"});
    let phemex = json!({"minAmountRv": "0"});
    let coinex = json!({"min_deposit_amount": "0.023"});
    let b = classify(
        arb::venues::bitget::num::dec_opt(&bitget, "minDepositAmount").map_err(|e| e.message),
    );
    let h = classify(arb::venues::htx::num::dec_opt(&htx, "minDepositAmt").map_err(|e| e.message));
    let p =
        classify(arb::venues::phemex::num::dec_opt(&phemex, "minAmountRv").map_err(|e| e.message));
    let c = classify(
        arb::venues::coinex::num::dec_opt(&coinex, "min_deposit_amount").map_err(|e| e.message),
    );
    assert_eq!(both(b), (Some(d("0.001")), Some(d("0.001"))));
    assert_eq!(both(h), (Some(d("0.0001")), Some(d("0.0001"))));
    // Phemex: «0» примера документации — минимум 0 (решение 370) при любой настройке.
    assert_eq!(both(p), (Some(Decimal::ZERO), Some(Decimal::ZERO)));
    assert_eq!(both(c), (Some(d("0.023")), Some(d("0.023"))));
}

#[test]
fn zero_absent_and_unreadable() {
    let parse = |v: Value| {
        classify(arb::venues::bitget::num::dec_opt(&v, "minDepositAmount").map_err(|e| e.message))
    };
    // «0» строкой и числом — минимум 0.
    for v in [
        json!({"minDepositAmount": "0"}),
        json!({"minDepositAmount": 0}),
    ] {
        assert_eq!(both(parse(v)), (Some(Decimal::ZERO), Some(Decimal::ZERO)));
    }
    // Поля нет и null — по настройке.
    for v in [json!({}), json!({"minDepositAmount": null})] {
        assert_eq!(both(parse(v)), (None, Some(Decimal::ZERO)));
    }
    // Не число и меньше нуля — неизвестен при любой настройке, пояснение в журнал.
    for v in [
        json!({"minDepositAmount": "abc"}),
        json!({"minDepositAmount": "-1"}),
    ] {
        let raw = parse(v);
        assert_eq!(both(raw.clone()), (None, None));
        assert!(note("minDepositAmount", &raw).is_some());
    }
    assert!(note("minDepositAmount", &parse(json!({"minDepositAmount": "0"}))).is_none());
}
