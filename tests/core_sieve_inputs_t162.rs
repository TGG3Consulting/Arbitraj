//! Файл: tests/core_sieve_inputs_t162.rs
//! Что проверяет: ревью t162, Н1, Н2, Н5 — сборка границы настоящим assemble (core/bound_in.rs) против ПОЛНОГО расчёта
//! (costs.rs evaluate_with, источник Live со снимком: вывод — costs_withdraw.rs и поправка по фактам withdraw_seen.rs,
//! возврат — return_b) на плотной сетке объёмов, на входах, которых не было в tests/core_sieve_t162.rs:
//! - доля комиссии вывода > 0 и налог перевода t > 0 и t < 0;
//! - комиссия покупки в котируемой монете (BookFee от суммы в USDT);
//! - комиссия продажи в монете ≥ 0 (граница её не вычитает);
//! - возврат USDT в каждой сделке (usdt_return per_trade: сеть BEP20 USDT BingX из записи config_getall.json);
//! - поправка вывода по фактам прошлого перевода (transfer.seen_cost = да, withdraw_seen record): фактическая комиссия
//!   меньше справочной с недостачей; поправка, дающая отрицательный фикс W; фикс справочника < 0 с поправкой;
//! - Н1: фикс вывода < 0 БЕЗ поправки по фактам — метод не действует (assemble — ошибка, кандидат в полный расчёт);
//! - Н2: комиссия продажи в монете < 0 — метод не действует.
//!
//! Свойство на каждом случае: U(q) ≥ прибыли полного расчёта в каждой допустимой точке сетки. Рынок (стакан BingX
//! CAKE-USDT, пул CAKE/USDT V2, правила пары, сети) — записи tests/size_common; доля, фикс, налог, ставки и факты
//! перевода — параметры сценария (в записях их нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{bound_in, costs, withdraw_seen}, tests/sieve_common, tests/size_common.

mod sieve_common;
mod size_common;

use arb::core::Core;
use arb::core::bound_in::{Seen, SideIn, assemble};
use arb::core::types::{Side, VenueId};
use arb::gateway::frozen::BookFee;
use arb::settings::scope::Scope;
use rust_decimal::Decimal;
use serde_json::json;
use sieve_common::{Books, Way, cake, covers, cx_with, dex_bound, frozen, frozen_with, usdt_net};
use size_common::{Rec, d};

/// Комиссия стороны снимка.
#[derive(Clone, Copy)]
enum Fee {
    /// Как адаптер BingX: покупка — в монете, продажа — в USDT, 0,1 %.
    Received,
    /// Покупка: ставка от суммы в USDT.
    BuyQuote(&'static str),
    /// Продажа: ставка от количества в монете.
    SellCoin(&'static str),
}

/// Случай проверки.
struct Case {
    name: &'static str,
    ways: &'static [Way],
    /// Фикс и доля вывода A (None — как в записи / без доли), налог перевода, %.
    fix: Option<&'static str>,
    pct: Option<&'static str>,
    tax: &'static str,
    fee: Fee,
    per_trade: bool,
    /// Факт прошлого перевода: (отправлено, зачислено, комиссия).
    seen: Option<(&'static str, &'static str, &'static str)>,
}

/// Снимки сторон по комиссии случая.
fn books(rec: &Rec, fee: Fee) -> Books {
    let (a, b) = match fee {
        Fee::Received => (frozen(rec, Side::Buy), frozen(rec, Side::Sell)),
        Fee::BuyQuote(r) => (
            frozen_with(rec, Side::Buy, plan(r, false, "USDT")),
            frozen(rec, Side::Sell),
        ),
        Fee::SellCoin(r) => (
            frozen(rec, Side::Buy),
            frozen_with(rec, Side::Sell, plan(r, true, "CAKE")),
        ),
    };
    Books { a, b }
}

/// План комиссии снимка. Ошибок нет.
fn plan(rate: &str, per_coin: bool, asset: &str) -> BookFee {
    BookFee {
        rate: d(rate),
        per_coin,
        asset: asset.into(),
        paid_in: None,
    }
}

/// transfer.seen_cost у BingX. Ошибок нет (паника теста).
async fn seen_cost(core: &Core, on: bool) {
    core.inner
        .deps
        .settings
        .set(
            "transfer.seen_cost",
            Scope::Venue("bingx".into()),
            Some(json!(on)),
            "тест",
        )
        .await
        .unwrap();
}

/// Один случай по всем его направлениям: граница настоящим assemble ≥ полного расчёта на сетке.
async fn run(core: &Core, rec: &Rec, c: &Case) {
    let mut net = rec.net.clone();
    if let Some(f) = c.fix {
        net.withdraw_fee = Some(d(f));
    }
    net.withdraw_fee_pct = c.pct.map(d);
    let mut seen = None;
    if let Some((sent, credited, fee)) = c.seen {
        let fact = arb::core::withdraw_seen::Seen {
            asset: cake(),
            from: VenueId("bingx".into()),
            sent: d(sent),
            credited: d(credited),
            fee: Some(d(fee)),
        };
        arb::core::withdraw_seen::record(core, fact).await;
        seen = Some(Seen {
            fee: d(fee),
            sent: d(sent),
        });
    }
    seen_cost(core, seen.is_some()).await;
    for w in c.ways {
        let mut cx = cx_with(rec, *w, d("0.0005"), d("1"), &net, d(c.tax));
        if c.per_trade {
            cx.return_per_trade = true;
            cx.usdt_b = Some(usdt_net());
        }
        let b = books(rec, c.fee);
        let (_, pts) = Box::pin(covers(core, rec, *w, &cx, &b, seen.as_ref())).await;
        println!("{} {w:?}: точек {}", c.name, pts.len());
        assert!(
            pts.len() > 20,
            "{} {w:?}: точек мало: {}",
            c.name,
            pts.len()
        );
    }
}

/// Случаи (см. шапку). Без фактов — первыми (поправка выключается настройкой).
fn cases() -> Vec<Case> {
    const ALL: &[Way] = &[Way::CexDex, Way::DexCex, Way::CexCex];
    const A_CEX: &[Way] = &[Way::CexDex, Way::CexCex];
    const B_CEX: &[Way] = &[Way::DexCex, Way::CexCex];
    let base = |name| Case {
        name,
        ways: ALL,
        fix: None,
        pct: None,
        tax: "0",
        fee: Fee::Received,
        per_trade: false,
        seen: None,
    };
    vec![
        Case {
            pct: Some("0.002"),
            tax: "3",
            ways: A_CEX,
            ..base("доля вывода и налог 3 %")
        },
        Case {
            tax: "-2",
            ways: A_CEX,
            ..base("налог −2 %")
        },
        Case {
            fee: Fee::BuyQuote("0.001"),
            ways: A_CEX,
            ..base("покупка в котируемой")
        },
        Case {
            fee: Fee::SellCoin("0.001"),
            ways: B_CEX,
            ..base("продажа в монете ≥ 0")
        },
        Case {
            per_trade: true,
            ways: B_CEX,
            ..base("возврат USDT в каждой сделке")
        },
        Case {
            seen: Some(("100", "99.5", "0.1")),
            ways: A_CEX,
            ..base("факт: комиссия ниже, недостача")
        },
        Case {
            pct: Some("0.002"),
            seen: Some(("100", "99.8", "0")),
            ways: A_CEX,
            ..base("факт: поправка даёт W < 0")
        },
        Case {
            fix: Some("-0.05"),
            seen: Some(("100", "99.9", "0.1")),
            ways: A_CEX,
            ..base("факт: фикс справочника < 0")
        },
    ]
}

/// Н5: граница настоящим assemble ≥ полного расчёта на всех случаях.
#[tokio::test]
async fn assembled_bound_covers_full_calc_on_inputs() {
    let dirp = std::env::temp_dir().join(format!("arb_sieve_in_t162_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dirp);
    std::fs::create_dir_all(&dirp).unwrap();
    let core = sieve_common::core(&dirp).await;
    let rec = Rec::load("sieve-in-t162").await;
    for c in cases() {
        Box::pin(run(&core, &rec, &c)).await;
    }
    let _ = std::fs::remove_dir_all(&dirp);
}

/// Сборка биржа → DEX на снимках `b` с сетью `net`. Ошибки: текст assemble.
fn try_cex_dex(
    rec: &Rec,
    b: &Books,
    net: &arb::core::types::CoinNetworkInfo,
    seen: Option<&Seen>,
) -> Result<(), String> {
    let c = cx_with(rec, Way::CexDex, d("0"), d("0"), net, Decimal::ZERO);
    let sa = b.a.price(d("1"));
    let db = dex_bound(rec, true);
    let a = SideIn::Book {
        book: &b.a.book,
        sample: &sa,
        ticker: "CAKE",
    };
    assemble(&c, &a, &SideIn::Dex(&db), seen).map(|_| ())
}

/// Н1, Н2: отрицательные входы, на которых граница была бы ниже расчёта, — метод не действует.
#[tokio::test]
async fn negative_inputs_are_not_applicable() {
    let rec = Rec::load("sieve-neg-t162").await;
    let b = books(&rec, Fee::Received);
    // Н1: фикс < 0 без поправки по фактам — ошибка; с поправкой (условия withdraw_seen model) — граница строится.
    let mut net = rec.net.clone();
    net.withdraw_fee = Some(d("-0.05"));
    let e = try_cex_dex(&rec, &b, &net, None).expect_err("фикс < 0 без поправки");
    assert!(e.contains("фикс"), "{e}");
    let s = Seen {
        fee: d("0.1"),
        sent: d("100"),
    };
    try_cex_dex(&rec, &b, &net, Some(&s)).expect("фикс < 0 с поправкой — строго");
    // Фикс ≥ 0 — как раньше.
    try_cex_dex(&rec, &b, &rec.net, None).expect("фикс записи");
    // Н2: продажа на бирже B с комиссией в монете < 0 — ошибка; ≥ 0 — строится.
    let c = cx_with(&rec, Way::DexCex, d("0"), d("0"), &rec.net, Decimal::ZERO);
    let da = dex_bound(&rec, false);
    for (rate, ok) in [("-0.001", false), ("0.001", true)] {
        let sell = frozen_with(&rec, Side::Sell, plan(rate, true, "CAKE"));
        let sb = sell.price(d("1"));
        let bi = SideIn::Book {
            book: &sell.book,
            sample: &sb,
            ticker: "CAKE",
        };
        let r = assemble(&c, &SideIn::Dex(&da), &bi, None);
        assert_eq!(
            r.is_ok(),
            ok,
            "ставка продажи в монете {rate}: {:?}",
            r.err()
        );
    }
}

/// Gate → BingX, журнал 7230071: фикс возврата 0.5, в докупке строка возврата 0.
/// На неизменном рынке обе ветви стоимости покрываются даже при смене группы.
#[tokio::test]
async fn group_return_bound_covers_both_costs() {
    use arb::core::bound_in::assemble_for_return;
    use arb::core::costs_withdraw::return_b_top_up;

    let rec = Rec::load("sieve-group-return").await;
    let mut c = cx_with(&rec, Way::DexCex, d("0"), d("0"), &rec.net, Decimal::ZERO);
    c.return_per_trade = true;
    let da = dex_bound(&rec, false);
    let sell = frozen(&rec, Side::Sell);
    let sample = sell.price(d("1"));
    let b = SideIn::Book {
        book: &sell.book,
        sample: &sample,
        ticker: "CAKE",
    };
    let net = usdt_net();
    for fix in [d("0.5"), Decimal::ZERO, d("-0.5")] {
        let mut tariff = net.clone();
        tariff.withdraw_fee = Some(fix);
        tariff.withdraw_fee_pct = None;
        c.usdt_b = Some(tariff.clone());
        let ordinary = assemble(&c, &SideIn::Dex(&da), &b, None).unwrap();
        let shared = assemble_for_return(&c, &SideIn::Dex(&da), &b, None, true).unwrap();
        let amount = d("410.304712572");
        let standalone_cost = tariff.fee_for(amount).unwrap();
        let grouped_cost = return_b_top_up(&c, amount).unwrap()[0].amount;
        assert_eq!(ordinary.fixed, standalone_cost);
        assert_eq!(shared.fixed, standalone_cost.min(grouped_cost));
        let q = d("1");
        assert_eq!(
            shared.u_at(q).unwrap().0 - ordinary.u_at(q).unwrap().0,
            standalone_cost - shared.fixed
        );
        if fix > Decimal::ZERO {
            let old = ordinary.best(None).unwrap();
            let new = shared.best(None).unwrap();
            let threshold = old.max_u.max(old.u0) + fix / Decimal::from(2);
            assert!(old.below_usd(threshold));
            assert!(!new.below_usd(threshold));
        }
        tariff.withdraw_fee_pct = Some(d("0.001"));
        c.usdt_b = Some(tariff.clone());
        let with_pct = assemble_for_return(&c, &SideIn::Dex(&da), &b, None, true).unwrap();
        assert!(with_pct.fixed <= tariff.fee_for(amount).unwrap());
        assert!(with_pct.fixed <= return_b_top_up(&c, amount).unwrap()[0].amount);
    }
}
