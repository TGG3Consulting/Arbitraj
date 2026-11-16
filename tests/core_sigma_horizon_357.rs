//! Файл: tests/core_sigma_horizon_357.rs
//! Что проверяет: решения 358 и 359 на записанных данных (моков нет):
//! - ряд σ — лучший бид Gate CAKE_USDT из записанного потока spot.order_book 100 мс с time_ms
//!   (tests/data/cex_samples_2026_11_04/gate_ws.jsonl), направление bsc_dex → gate, как tests/core_price_reserve.rs;
//! 1) σ ряда (vol.rs measure) после перехода на хранение доходности со знаком — та же, что Σ(ln pᵢ/pᵢ₋₁)²/ΣΔt, посчитанная
//!    здесь заново по ценам; число ненулевых изменений — то же;
//! 2) σ на горизонте T (vol_horizon.rs через Volatility::horizon) — та же, что независимый счёт по ценам: цена конца
//!    каждого отрезка длины T назад от «сейчас» — последний замер не позже него, Rⱼ = ln(P конца / P начала),
//!    σ_T = √(ΣRⱼ²/(m·T)); ρ₁ = Σrᵢrᵢ₋₁/Σrᵢ²;
//! 3) reserve_check::of (решение 364 — всегда, без настройки): σ ряда и σ_T те же, что у vol.rs; по копии одного ряда
//!    (Volatility::copy_of — счёт вне замка ядра) — то же сравнение; граница
//!    profit.sigma_floor_per_s [B] — σ_T плана = max(σ_T, граница), σ ряда не меняется;
//! 4) cand_signal_note::data: прежнее поле data (Calc записанного расчёта CAKE, tests/size_common) не меняется, к нему
//!    добавлено signal_note; строки сигнала нет — signal = null; возраст котировок расчёта — мс от их времени до расчёта.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{vol, vol_horizon, reserve_check, cand_signal_note}, tests/size_common.

mod size_common;

use arb::core::Core;
use arb::core::cand_signal_note;
use arb::core::deps::Deps;
use arb::core::reserve_check;
use arb::core::types::{Asset, Direction, Side, VenueId};
use arb::core::vol::Volatility;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::{DateTime, Duration, TimeZone, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, MathematicalOps};
use serde_json::{Value, json};
use size_common::{CAKE, Rec, d};
use tokio::sync::broadcast;

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Записанный лучший бид Gate CAKE_USDT: (time_ms кадра, цена) по кадрам spot.order_book «update».
fn gate_bids() -> Vec<(DateTime<Utc>, Decimal)> {
    let raw = std::fs::read_to_string("tests/data/cex_samples_2026_11_04/gate_ws.jsonl").unwrap();
    raw.lines()
        .filter_map(|l| {
            let o: Value = serde_json::from_str(l).unwrap();
            let m: Value = serde_json::from_str(o["msg"].as_str()?).unwrap();
            (m["channel"] == "spot.order_book" && m["event"] == "update").then(|| {
                let t = Utc
                    .timestamp_millis_opt(m["time_ms"].as_i64().unwrap())
                    .unwrap();
                (t, d(m["result"]["bids"][0][0].as_str().unwrap()))
            })
        })
        .collect()
}

/// Замеры, которые примет ряд vol.rs: время строго вперёд (как Volatility::sample).
fn kept(frames: &[(DateTime<Utc>, Decimal)]) -> Vec<(DateTime<Utc>, Decimal)> {
    let mut out: Vec<(DateTime<Utc>, Decimal)> = vec![];
    for (t, p) in frames {
        if out.last().is_none_or(|(lt, _)| t > lt) {
            out.push((*t, *p));
        }
    }
    out
}

fn series(frames: &[(DateTime<Utc>, Decimal)]) -> Volatility {
    let mut vol = Volatility::default();
    for (t, p) in frames {
        vol.sample(
            &v("gate"),
            &cake(),
            Side::Sell,
            &v("bsc_dex"),
            *p,
            *t,
            Some(60),
        );
    }
    vol
}

fn secs(a: DateTime<Utc>, b: DateTime<Utc>) -> Decimal {
    Decimal::from((a - b).num_milliseconds()) / Decimal::ONE_THOUSAND
}

#[test]
fn horizon_on_recorded_gate() {
    let frames = gate_bids();
    let k = kept(&frames);
    assert!(k.len() > 20, "кадров мало: {}", k.len());
    let now = k.last().unwrap().0;
    let mut vol = series(&frames);
    // 1) σ ряда — как раньше.
    let r: Vec<Decimal> = k.windows(2).map(|w| (w[1].1 / w[0].1).ln()).collect();
    let dt: Decimal = k.windows(2).map(|w| secs(w[1].0, w[0].0)).sum();
    let sq: Decimal = r.iter().map(|x| x * x).sum();
    let (sigma, changes) = vol
        .measure(&v("gate"), &cake(), Side::Sell, &v("bsc_dex"), 60, now)
        .unwrap();
    assert_eq!(sigma, (sq / dt).sqrt().unwrap());
    assert_eq!(changes, r.iter().filter(|x| !x.is_zero()).count());
    // 2) σ на горизонте — по ценам концов отрезков.
    let price_at = |g: DateTime<Utc>| k.iter().rev().find(|(t, _)| *t <= g).unwrap().1;
    for h in [d("1"), d("2"), d("4")] {
        let got = vol
            .horizon(&v("gate"), &cake(), Side::Sell, &v("bsc_dex"), 60, now, h)
            .unwrap();
        let m = (secs(now, k[0].0) / h).floor();
        let m_n = m.to_usize().unwrap();
        assert_eq!(got.blocks, m_n);
        let step = Duration::milliseconds((h * Decimal::ONE_THOUSAND).to_i64().unwrap());
        let total: Decimal = (1..=m_n)
            .map(|j| {
                let end = now - step * (j as i32 - 1);
                let x = (price_at(end) / price_at(end - step)).ln();
                x * x
            })
            .sum();
        let want = (total / (m * h)).sqrt().unwrap();
        let diff = (got.sigma_t.unwrap() - want).abs();
        assert!(
            diff < d("0.000000000000001"),
            "T = {h}: {} против {want}",
            got.sigma_t.unwrap()
        );
        assert_eq!(got.steps, r.len());
        assert_eq!(got.changes, changes);
        let cross: Decimal = r.windows(2).map(|w| w[0] * w[1]).sum();
        assert_eq!(got.rho1, Some(cross / sq));
    }
    // Горизонт длиннее ряда — отрезков нет, σ_T нет.
    let long = vol
        .horizon(
            &v("gate"),
            &cake(),
            Side::Sell,
            &v("bsc_dex"),
            60,
            now,
            d("3600"),
        )
        .unwrap();
    assert_eq!((long.blocks, long.sigma_t), (0, None));
}

/// Настоящее ядро без площадок и хранилище настроек (как tests/core_price_reserve.rs).
async fn core(name: &str) -> (Core, Store) {
    let p = std::env::temp_dir().join(format!("arb_sigma357_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    let db = Db::open(&p.join("arb.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(p.join("secrets.enc")),
        journal: journal.clone(),
    };
    let deps = Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: p.clone(),
    };
    (Core::start(deps).await, settings)
}

async fn set(s: &Store, name: &str, scope: Scope, v: Value) {
    s.set(name, scope, Some(v), "тест").await.unwrap();
}

#[tokio::test]
async fn check_always_and_floor() {
    let (_core, s) = core("check").await;
    set(&s, "profit.sigma_window_min", Scope::Global, json!(60)).await;
    let frames = gate_bids();
    let now = frames.last().unwrap().0;
    let gd = Direction {
        asset: cake(),
        buy_on: v("bsc_dex"),
        sell_on: v("gate"),
    };
    let mut vol = series(&frames);
    let plan = d("0.00001");
    // 3) Решение 364: сравнение всегда, без настройки; по копии одного ряда (вне замка ядра) — те же поля, что по рядам.
    let c = reserve_check::of(&mut vol, &s, &gd, plan, d("2"), now);
    let mut one = vol.copy_of(&v("gate"), &cake(), Side::Sell, &v("bsc_dex"));
    let c1 = reserve_check::of(&mut one, &s, &gd, plan, d("2"), now);
    assert_eq!(format!("{c:?}"), format!("{c1:?}"));
    let (sigma, _) = vol
        .measure(&v("gate"), &cake(), Side::Sell, &v("bsc_dex"), 60, now)
        .unwrap();
    let h = vol
        .horizon(
            &v("gate"),
            &cake(),
            Side::Sell,
            &v("bsc_dex"),
            60,
            now,
            d("2"),
        )
        .unwrap();
    assert_eq!(c.sigma_plan, plan);
    assert_eq!(c.sigma_series, Some(sigma));
    assert_eq!(c.sigma_t_plan, h.sigma_t);
    assert_eq!(c.variance_ratio, Some((h.sigma_t.unwrap() / sigma).powi(2)));
    assert_eq!((c.reserve_t_usdt, c.profit_t_usd), (None, None));
    // Граница выше σ_T — σ_T плана = граница; σ ряда та же.
    let floor = h.sigma_t.unwrap() * d("10");
    set(
        &s,
        "profit.sigma_floor_per_s",
        Scope::Venue("gate".into()),
        json!(floor.to_string()),
    )
    .await;
    let c = reserve_check::of(&mut vol, &s, &gd, plan, d("2"), now);
    assert_eq!(c.sigma_t_plan, Some(floor));
    assert_eq!(c.sigma_series, Some(sigma));
}

#[tokio::test]
async fn reject_data_keeps_calc_and_adds_note() {
    let (core, _s) = core("note").await;
    let rec = Rec::load("sigma357_note").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    let st = cand_signal_note::start(&core, &calc.direction);
    // 4) Отказ с расчётом: прежние поля Calc те же, плюс signal_note.
    let got = cand_signal_note::data(Some(&calc), &st).unwrap();
    let mut was = serde_json::to_value(&calc).unwrap();
    let note = got["signal_note"].clone();
    was["signal_note"] = note.clone();
    assert_eq!(got, was);
    assert_eq!(note["signal"], Value::Null);
    let age = (calc.at - calc.buy_quote.at).num_milliseconds();
    assert_eq!(note["calc"]["age_at_calc_ms"]["buy"], json!(age));
    assert_eq!(note["calc"]["qty"], json!(calc.qty));
    // Отказ без расчёта — только signal_note.
    let none = cand_signal_note::data(None, &st).unwrap();
    assert_eq!(none["signal_note"]["calc"], Value::Null);
    assert_eq!(none.as_object().unwrap().len(), 1);
}
