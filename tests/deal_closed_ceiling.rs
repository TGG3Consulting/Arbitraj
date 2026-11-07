//! Файл: tests/deal_closed_ceiling.rs
//! Что проверяет: решение 216 — строки расчёта «без потолка» в сообщении Telegram о закрытой сделке (deal.closed):
//! плюсовая с записью («💸 Упустили», «🔝 Потолок: вложить → прибыль»), плюсовая с оптимумом, не проходящим пороги
//! («(не проходит пороги)»), минусовая с записью («🔝 Потолок», «💥 Через потолок потеряли бы ≈» — тот же процент
//! убытка на объёме потолка), без записи («Без потолка: не посчитано» и причина).
//! Данные: запись «без потолка» считается настоящим расчётом ядра (core/size_uncapped.rs) на записанных данных CAKE
//! (tests/size_common: стакан BingX ws_spot_2026-10-23, пул CAKE/USDT V2 на блоке 0x7b936ae), упущено — uncapped_types::
//! missed; запись проходит настоящую SQLite (storage/uncapped_db.rs insert → by_deal по id сделки) и разбор
//! core/deal_ceiling.rs. Суммы сделки (вложено, выручка) — бюджет и прибыль того же расчёта на боевом бюджете.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{deal, deal_ceiling, deal_text, size_uncapped, uncapped_types}, arb::storage, tests/size_common.

mod size_common;

use arb::core::deal::{Deal, Step};
use arb::core::deal_ceiling::{self, Ceiling};
use arb::core::deal_text::closed;
use arb::core::size_uncapped::{At, uncapped};
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::uncapped_types::missed;
use arb::storage::db::Db;
use arb::storage::uncapped_db::{self, Row};
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};

/// id сделки теста (формат ledger::open: время до мс + тикер).
const ID: &str = "20261023120000000CAKE";

/// Сделка CAKE DEX → BingX, закрыта (затрат сверх выручки нет). Принимает: прибыль плана, вложено, выручку.
fn deal(plan: Decimal, invested: Decimal, proceeds: Decimal) -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bsc_dex".into()),
        sell_on: VenueId("bingx".into()),
    };
    let mut x = Deal::new(ID.into(), dir, serde_json::json!({}), plan);
    x.at = VenueId("bingx".into());
    x.invested = invested;
    x.proceeds = proceeds;
    x.step = Step::Closed;
    x.result_usd = Some(proceeds - invested);
    x
}

/// Запись «без потолка» сделки на записанном CAKE (боевой бюджет 100 USDT) через настоящую SQLite. Принимает: проверку
/// порогов точки, метку папки базы (тесты идут параллельно). Возвращает: (расчёт из базы, лучшее на боевом бюджете).
async fn record(pass: fn(Decimal) -> bool, tag: &str) -> (Ceiling, At) {
    let rec = Rec::load("deal-ceiling").await;
    let mut c = RecCurve {
        rec: &rec,
        dir: Dir::DexToBingx,
        slip: Decimal::ONE,
    };
    let p = params(&rec, Dir::DexToBingx, d("1000000"));
    let u = uncapped(&mut c, &p, d("100"), &[], |pt| pass(pt.profit))
        .await
        .unwrap();
    let opt = At::of(&u.opt);
    let body = serde_json::json!({
        "opt": opt,
        "opt_passes": u.opt_passes,
        "missed": missed("deal", u.opt_passes, opt.profit, &u.at_budget),
    });
    let dir = std::env::temp_dir().join(format!("arb_deal_ceiling_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    uncapped_db::insert(
        &db,
        Row {
            kind: "deal",
            deal_id: Some(ID.into()),
            ticker: "CAKE".into(),
            address: None,
            started_at: "2026-10-23T12:00:00Z".into(),
            body: body.to_string(),
        },
    )
    .await
    .unwrap();
    let (_, b) = uncapped_db::by_deal(&db, ID.into()).await.unwrap().unwrap();
    // Чужой id — записи нет.
    assert!(
        uncapped_db::by_deal(&db, "x".into())
            .await
            .unwrap()
            .is_none()
    );
    (deal_ceiling::parse(&b), u.at_budget.at.unwrap())
}

#[tokio::test]
async fn plus_with_record() {
    let (c, at) = record(|p| p >= Decimal::ZERO, "plus").await;
    let Ceiling::Known {
        invested,
        passes,
        missed,
        ..
    } = c.clone()
    else {
        panic!("{c:?}")
    };
    assert!(
        passes && invested > at.invested && missed > Decimal::ZERO,
        "{c:?}"
    );
    let msg = closed(
        &deal(at.profit, at.invested, at.invested + at.profit),
        "продано",
        Some(&c),
    );
    println!("{msg}\n");
    assert_eq!(
        msg,
        "✅ Сделка плюсовая\nCAKE DEX → BingX\nРазмер ордера: 100.00 USDT\nПлан: +5.73 USDT\nФакт: +5.73 USDT\n\
         Заработали: 5.73 USDT\n💸 Упустили: 342.27 USDT\n🔝 Потолок: 12309.68 USDT → прибыль 347.99 USDT\n\nid 20261023120000000CAKE"
    );
}

#[tokio::test]
async fn plus_opt_below_thresholds() {
    // Порог выше любой прибыли: оптимум не проходит пороги, упущено 0 (uncapped_types::missed).
    let (c, at) = record(|p| p >= d("1000000000"), "below").await;
    let msg = closed(
        &deal(at.profit, at.invested, at.invested + at.profit),
        "продано",
        Some(&c),
    );
    println!("{msg}\n");
    assert!(msg.contains("💸 Упустили: 0.00 USDT\n"), "{msg}");
    assert!(
        msg.contains("→ прибыль 347.99 USDT (не проходит пороги)\n"),
        "{msg}"
    );
}

#[tokio::test]
async fn minus_with_record() {
    let (c, at) = record(|p| p >= Decimal::ZERO, "minus").await;
    // Минусовая: вложено как у расчёта на бюджете, продано на 1.5 USDT дешевле.
    let x = deal(at.profit, at.invested, at.invested - d("1.5"));
    let msg = closed(&x, "продано", Some(&c));
    println!("{msg}\n");
    let Ceiling::Known { invested, .. } = c else {
        unreachable!()
    };
    // Тот же процент убытка на объёме потолка: −1.5 / вложено × вложение потолка.
    let est = deal_ceiling::loss_at(d("-1.5"), at.invested, invested).unwrap();
    assert_eq!(est, d("-1.5") / at.invested * invested);
    assert_eq!(
        msg,
        "❌ Сделка минусовая\nCAKE DEX → BingX\nРазмер ордера: 100.00 USDT\nПлан: +5.73 USDT\nФакт: −1.50 USDT\n\
         Итого минус: 1.50 USDT\n🔝 Потолок: 12309.68 USDT\n💥 Через потолок потеряли бы ≈ 184.65 USDT\n\nid 20261023120000000CAKE"
    );
}

#[test]
fn without_record() {
    let x = deal(d("0.13"), d("45"), d("45.1"));
    let msg = closed(&x, "продано", Some(&Ceiling::Unknown(None)));
    assert!(
        msg.ends_with(
            "Заработали: 0.10 USDT\nБез потолка: не посчитано\n\nid 20261023120000000CAKE"
        ),
        "{msg}"
    );
    let why = Ceiling::Unknown(Some("ждёт в очереди расчёта".into()));
    let msg = closed(&deal(d("0.02"), d("45"), d("44.9")), "продано", Some(&why));
    assert!(
        msg.contains("\nБез потолка: не посчитано (ждёт в очереди расчёта)\n"),
        "{msg}"
    );
    // Непонятный JSON — тоже «не посчитано» с причиной.
    assert!(matches!(
        deal_ceiling::parse("{}"),
        Ceiling::Unknown(Some(_))
    ));
    // Вложено 0 — процента убытка нет, оценки нет.
    assert_eq!(
        deal_ceiling::loss_at(d("-1"), Decimal::ZERO, d("100")),
        None
    );
}
