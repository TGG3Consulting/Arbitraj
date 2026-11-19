//! Файл: tests/core_credit_rest_live_t198.rs
//! Что делает: решение 413 (t198, после шестого ревью) — решение остатка «не присвоено» под замком счёта при
//! ПРОЧИТАННОМ балансе биржи (стенд tests/t198_live: настоящий адаптер XT, хранилище открыто, баланс BTC — из ячейки):
//! credit_rest::locked — тихо → добор (и строка журнала «взято сверх зачисления … решение 76»), не тихо → «счёт занят»;
//! круг сверки credit_rest_park::round — тихо → решено, на счёте ничего → фантом закрыт, человек нажал кнопку во время
//! чтения → его действие не перетирается; «Продать сейчас» по алерту (manual::sell_now) — через locked: тихо → продажа
//! с добором, не тихо → «счёт занят», ничего → фантом закрыт. Моков нет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core; tests/t198_common, tests/t198_live.

mod t198_common;
mod t198_live;

use arb::core::Core;
use arb::core::credit_fact::{Basis, confirm, rest, set_rest};
use arb::core::credit_rest::Res;
use arb::core::deal::{Deal, Step};
use t198_common::{credited, d};
use t198_live::{big, start};

/// Сделка BTC на xt: запись 1, подтверждено `qty`, остаток `r`, шаг `step`.
fn deal(id: &str, qty: &str, r: &str, step: Step) -> Deal {
    let mut a = credited(id, "xt", d("1"));
    a.direction.asset.ticker = "BTC".into();
    confirm(&mut a, d(qty), Basis::Balance);
    set_rest(&mut a, d(r));
    a.step = step;
    a
}

/// Сделка из учёта памяти.
fn mem(core: &Core, id: &str) -> Deal {
    core.inner.deals.lock().unwrap().get(id).cloned().unwrap()
}

/// Другая сделка BTC на xt, ждущая доли (счёт не тихий).
async fn busy_neighbor(core: &Core, id: &str) {
    let mut p = credited(id, "xt", d("0.1"));
    p.direction.asset.ticker = "BTC".into();
    arb::core::ledger::save(core, &p).await;
}

/// locked: тихо — добор min(0.5 + 0.3, 1) = 0.8, сверх ожидаемого зачисления (замер 50 %: 0.5) — строка журнала;
/// не тихо — «занят».
#[test]
fn locked_reads_and_resolves() {
    big(async {
        let (core, _) = start(&t198_common::dir("live-locked"), "1").await;
        let mut a = deal("L1", "0.5", "0.3", Step::Selling);
        let s = arb::core::deposit_hold::Sample {
            records: d("1"),
            credited: d("0.5"),
            pct: d("50"),
            at: chrono::Utc::now(),
            deals: vec!["L0".into()],
        };
        arb::core::deposit_hold::put(&core, &a.at, &a.direction.asset, s);
        arb::core::ledger::save(&core, &a).await;
        assert!(matches!(
            arb::core::credit_rest::locked(&core, &mut a).await,
            Res::Done
        ));
        assert_eq!((a.qty, rest(&a)), (d("0.8"), None));
        assert_eq!(mem(&core, "L1").qty, d("0.8"), "учёт памяти — сразу");
        let all = core.inner.deps.journal.list(500, None).await.unwrap();
        assert!(all.iter().any(|e| e.kind == "core.credit_fact"
            && e.message.contains("сверх зачисления")
            && e.message.contains("решение 76")));
        let mut b = deal("L2", "0.2", "0.3", Step::Selling);
        arb::core::ledger::save(&core, &b).await;
        busy_neighbor(&core, "P2").await;
        assert!(matches!(
            arb::core::credit_rest::locked(&core, &mut b).await,
            Res::Busy(_)
        ));
        assert_eq!(rest(&b), Some(d("0.3")));
    });
}

/// Круг сверки: тихо — решено (остановлена с 0.5); на счёте ничего — фантом закрыт.
#[test]
fn round_resolves_and_closes_phantom() {
    big(async {
        let (core, stand) = start(&t198_common::dir("live-round"), "1").await;
        let a = deal("R1", "0.2", "0.3", Step::Halted);
        arb::core::ledger::save(&core, &a).await;
        arb::core::credit_rest_park::round(&core, &a.at).await;
        let r = mem(&core, "R1");
        assert_eq!((r.step, r.qty, rest(&r)), (Step::Halted, d("0.5"), None));
        *stand.btc.lock().unwrap() = "0".into();
        let b = deal("R2", "0", "0.3", Step::Halted);
        arb::core::ledger::save(&core, &b).await;
        arb::core::credit_rest_park::round(&core, &b.at).await;
        assert_eq!(mem(&core, "R2").step, Step::Closed, "фантом закрыт");
    });
}

/// Круг сверки: человек нажал «Беру на себя» во время чтения баланса — шаг человека не перетирается.
#[test]
fn round_keeps_human_action() {
    big(async {
        let (core, stand) = start(&t198_common::dir("live-human"), "1").await;
        *stand.delay_ms.lock().unwrap() = 500;
        let a = deal("H1", "0.2", "0.3", Step::Halted);
        arb::core::ledger::save(&core, &a).await;
        let c = core.clone();
        let v = a.at.clone();
        let h = tokio::spawn(async move { arb::core::credit_rest_park::round(&c, &v).await });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let mut human = a.clone();
        human.step = Step::TakenOver;
        arb::core::ledger::save(&core, &human).await;
        h.await.unwrap();
        assert_eq!(mem(&core, "H1").step, Step::TakenOver);
    });
}

/// «Продать сейчас» по алерту: не тихо — «счёт занят», алерт остаётся; тихо — добор и продажа; ничего — фантом.
#[test]
fn sell_now_by_alert_goes_through_locked() {
    big(async {
        let (core, stand) = start(&t198_common::dir("live-alert"), "1").await;
        let a = deal("S1", "0.2", "0.3", Step::Selling);
        arb::core::ledger::save(&core, &a).await;
        arb::core::manual::raise_alert(&core, a, None, "тест".into(), None).await;
        let id = alert_of(&core, "S1");
        busy_neighbor(&core, "P3").await;
        let e = arb::core::manual::sell_now(&core, &id, false)
            .await
            .unwrap_err();
        assert!(e.contains("счёт занят"), "{e}");
        assert_eq!(alert_of(&core, "S1"), id, "алерт остаётся");
        let mut p = mem(&core, "P3");
        p.step = Step::Closed;
        arb::core::ledger::save(&core, &p).await;
        assert!(arb::core::manual::sell_now(&core, &id, false).await.is_ok());
        let s = mem(&core, "S1");
        assert_eq!((s.qty, rest(&s)), (d("0.5"), None), "добрано до продажи");
        *stand.btc.lock().unwrap() = "0".into();
        let b = deal("S2", "0", "0.3", Step::Selling);
        arb::core::ledger::save(&core, &b).await;
        arb::core::manual::raise_alert(&core, b, None, "тест".into(), None).await;
        let id = alert_of(&core, "S2");
        let ok = arb::core::manual::sell_now(&core, &id, false)
            .await
            .unwrap();
        assert!(ok.contains("закрыта"), "{ok}");
        assert_eq!(mem(&core, "S2").step, Step::Closed);
    });
}

/// ID алерта сделки.
fn alert_of(core: &Core, deal: &str) -> String {
    let m = core.inner.alerts.lock().unwrap();
    m.values()
        .find(|(_, d)| d.id == deal)
        .map(|(a, _)| a.id.clone())
        .unwrap()
}
