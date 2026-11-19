//! Файл: tests/core_credit_review3b_t198.rs
//! Что делает: решение 413 (третье ревью t198), без площадок и моков: замок счёта — чтение факта ждёт конца продажи,
//! снимок посторонних не ждёт (С-А, С-Ж); монеты уходящей со счёта сделки вычитаются из пула ждущих (С-Е); круг сверки
//! подтверждает только монеты, чьи ждущие без задачи не менялись за запрос баланса (С-Б); неизвестный basis записи —
//! «ждёт доли» (М-5); по исходникам без комментариев: вывод — после факта, первый вывод ждёт и перечитывает (С-Д),
//! чтение баланса без побочных действий правил шлюза (С-З), снимок запечатывается сразу после ответа на вывод (С-Ж),
//! свежий факт сбрасывается в режиме застрявшей монеты и в алерте (Б-1).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core; tests/t198_common.

mod t198_common;

use std::time::Duration;

use arb::core::credit_lock;
use arb::core::deal::Step;
use arb::core::types::{Asset, VenueId};
use t198_common::{code_of, credited, d, start};

/// С-А, С-Ж: пока продажа держит счёт, чтение факта ждёт (без предела), снимок (fact_try) — нет; конец продажи —
/// чтение идёт.
#[tokio::test]
async fn fact_waits_for_sale_snapshot_does_not() {
    let v = VenueId("mexc".into());
    let a = Asset {
        chain: None,
        ticker: "LOCK198".into(),
        contract: None,
    };
    let sale = credit_lock::sale(&v, &a).await;
    assert!(credit_lock::fact_try(&v, &a).is_none(), "снимок не ждёт");
    let (v2, a2) = (v.clone(), a.clone());
    let h = tokio::spawn(async move {
        let _g = credit_lock::fact(&v2, &a2).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!h.is_finished(), "факт ждёт конца продажи");
    drop(sale);
    tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .expect("после продажи факт читается")
        .unwrap();
    assert!(credit_lock::fact_try(&v, &a).is_some());
}

/// С-Е: сделка на шаге «перевод», уходящая с этого счёта, — в leaving с количеством; распределение вычитает её
/// количество из пула ждущих и считает замер неоднозначным.
#[tokio::test]
async fn leaving_deal_qty_is_taken_from_pool() {
    let core = start("leaving").await;
    let mexc = VenueId("mexc".into());
    let a = credited("A1", "mexc", d("100"));
    arb::core::ledger::save(&core, &a).await;
    let mut l = credited("L1", "mexc", d("50"));
    l.credit = None;
    l.step = Step::Transfer;
    l.transit_to = Some(VenueId("bingx".into()));
    arb::core::ledger::save(&core, &l).await;
    let o = arb::core::remain_others::on_account(&core, &a, &mexc);
    assert_eq!(
        (o.leaving.clone(), o.leaving_qty),
        (vec!["L1".to_string()], d("50"))
    );
    assert!(o.unclear.is_empty(), "уходит — не «в пути сюда»");
    let i = arb::core::credit_apply::input(&core, &a, d("150"), &o);
    assert_eq!(i.confirmed, d("50"));
    assert!(i.unclear);
    let sp = arb::core::credit_split::split(&i);
    assert_eq!(sp.share_of("A1"), Some(d("100")));
    assert!(!sp.measurable);
}

/// С-Б: стабильны только монеты, у которых отпечатки ждущих без задачи до и после запроса баланса одинаковы.
#[test]
fn reconcile_round_takes_only_stable_coins() {
    use arb::core::credit_halted::stable;
    let t0 = chrono::Utc::now();
    let t1 = t0 + chrono::Duration::seconds(1);
    let before = vec![
        ("WKC".to_string(), "A".to_string(), Step::Halted, t0),
        ("XYZ".to_string(), "B".to_string(), Step::Halted, t0),
    ];
    let after = vec![
        ("WKC".to_string(), "A".to_string(), Step::Halted, t0),
        ("XYZ".to_string(), "B".to_string(), Step::WaitingHuman, t1),
    ];
    assert_eq!(stable(&before, &after), vec!["WKC".to_string()]);
    let late = vec![("NEW".to_string(), "C".to_string(), Step::Halted, t0)];
    assert!(
        stable(&before, &late).is_empty(),
        "появилась за запрос — не сверяем"
    );
}

/// М-5: неизвестный basis при чтении записи — «ждёт доли» (как запись), а не ошибка чтения сделки.
#[test]
fn unknown_basis_reads_as_pending() {
    let a = credited("M5", "mexc", d("100"));
    let mut c = serde_json::to_value(a.credit.as_deref().unwrap()).unwrap();
    c["basis"] = serde_json::json!("Future");
    let back: arb::core::credit_fact::CreditFact = serde_json::from_value(c).unwrap();
    assert_eq!(back.basis, None);
}

/// По исходникам без комментариев: где стоит поведение третьего ревью.
#[test]
fn sources_keep_review3_rules() {
    // Б2 / Б-2: вывод и оценка перегона — после факта.
    for f in [
        "src/core/executor.rs",
        "src/core/relay.rs",
        "src/core/manual_relay.rs",
    ] {
        let src = code_of(f);
        let fact = src
            .find("sell_fact::before(")
            .unwrap_or_else(|| panic!("{f}: нет факта"));
        for what in [
            "transfers::send_trade(",
            "relay_value::value_at(",
            "relay_to(core",
        ] {
            for (at, _) in src.match_indices(what) {
                assert!(fact < at, "{f}: {what} раньше факта");
            }
        }
    }
    // С-Д: первый вывод — режим «ждать», ожидание с перечитыванием.
    assert!(code_of("src/core/executor.rs").contains("sell_fact::WITHDRAW"));
    // Поведение ожидания (предел, выход при человеке) — tests/core_credit_review4_t198.rs.
    assert!(code_of("src/core/remain_fact.rs").contains("fact_wait::first("));
    // С-З: чтение баланса — без rules_apply (журнал Error, Telegram, ошибки подряд, «стоп»).
    assert!(!code_of("src/core/remain_read.rs").contains("rules_apply"));
    // С-Ж: снимок запечатан до стадии «отправлено».
    let t = code_of("src/core/transfers.rs");
    let seal = t.find("credit_base::seal(").unwrap();
    assert!(seal < t.find("Stage::Sent").unwrap());
    // Б-1: свежий факт сбрасывается в режиме застрявшей монеты и в алерте до первого ожидания.
    for (f, at) in [
        ("src/core/stuck.rs", "fn handle_as_is"),
        ("src/core/manual.rs", "fn raise_alert_with"),
    ] {
        let src = code_of(f);
        let start = src.find(at).unwrap();
        let body = &src[start..];
        let reset = body
            .find("fact_fresh = None")
            .unwrap_or_else(|| panic!("{f}: нет сброса"));
        assert!(
            body.find(".await").is_none_or(|w| reset < w),
            "{f}: сброс после ожидания"
        );
    }
}
