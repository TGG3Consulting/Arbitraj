//! Файл: tests/core_price_reserve.rs
//! Что проверяет: решение 240 — запас на движение цены (core/reserve.rs) на живом ядре с SQLite (настоящий Core,
//! настройки в хранилище) и записанных данных:
//! - ряд σ — лучший бид Gate CAKE_USDT из записанного потока spot.order_book (100 мс) с временем time_ms
//!   (tests/data/cex_samples_2026_11_04/gate_ws.jsonl), направление bsc_dex → gate (продажа на Gate);
//! - расчёт плана — та же модель (costs.rs evaluate_with) над источником пересчёта по окну (uncapped_replay.rs): покупка
//!   по записанному стакану BingX CAKE-USDT, продажа на DEX — котировка плана из записанного пула CAKE/USDT V2
//!   (tests/size_common/calc.rs); биржа ↔ биржа — стакан BingX на обеих сторонах (как tests/uncapped_decided.rs).
//!   На этих записях пул дешевле стакана (биржа → DEX — спред −6 %), поэтому доля закрытия проверяется на DEX → BingX:
//!   источник PoolToBook собран из функций системы над записями (как tests/size_common/curve.rs DexToBingx): покупка —
//!   обмен адаптера DEX в записанном пуле (pool_quote::swap, точный выход), продажа — снимок стакана BingX (gateway/frozen.rs
//!   FrozenBook: котировка и худшая цена), вывод с DEX и курсы — затраты плана (core/plan_costs.rs; газа и ставок в
//!   записях нет — затрат сверх цены нет, комиссия стакана 0).
//! 1) Без настроек — как раньше: σ та же, что vol.rs sigma; доли нет; запас = k·σ·√T·выручка тем же выражением, расчёт
//!    побайтно тот же, что у контекста без доли.
//! 2) profit.sigma_floor_per_s [B]: σ = max(σ ряда, граница); запас растёт ровно до k·граница·√T·выручка.
//! 3) profit.sigma_min_changes: меньше ненулевых изменений цены в окне — σ не измерена (причина); σ = 0 при минимуме 1 —
//!    сделки нет.
//! 4) profit.closure_share [B]: запас = max(k·σ·√T·выручка, доля · спред · выручка), прибыль меньше ровно на разницу
//!    запасов; спред ≤ 0 (биржа ↔ биржа на одном стакане) — запас прежний; доля другой площадки не действует.
//! 5) Копия настроек на момент входа (решение 239): изменение после входа доли, границы и минимума ничего не меняет.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{reserve, vol, costs, uncapped_replay}, tests/size_common. Моков нет.

mod size_common;

use arb::core::Core;
use arb::core::convert::Rates;
use arb::core::costs::{self, Calc, Source};
use arb::core::costs_quote::classify;
use arb::core::deps::Deps;
use arb::core::dirctx::DirCtx;
use arb::core::plan_costs::PlanCosts;
use arb::core::registry::Presence;
use arb::core::reserve;
use arb::core::size::Fail;
use arb::core::types::{Asset, Cost, CostKind, Direction, OrderBook, Quote, Side, VenueId};
use arb::core::uncapped_replay::{Plan, Replay};
use arb::core::vol::Volatility;
use arb::gateway::frozen::{BookFee, FrozenBook, deepest};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::{Decimal, MathematicalOps};
use serde_json::{Value, json};
use size_common::{CAKE, Rec, USDT, d};
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

fn dir(a: &str, b: &str) -> Direction {
    Direction {
        asset: cake(),
        buy_on: v(a),
        sell_on: v(b),
    }
}

/// Настоящее ядро без площадок (как tests/uncapped_decided.rs).
async fn core(name: &str) -> (Core, Store) {
    let p = std::env::temp_dir().join(format!("arb_reserve_{name}_{}", std::process::id()));
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

async fn unset(s: &Store, name: &str, scope: Scope) {
    s.set(name, scope, None, "тест").await.unwrap();
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

/// Ряд σ направления bsc_dex → gate из кадров; окно 60 мин.
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

/// σ без настроек решения 240 (vol.rs sigma, как до решения).
fn old_sigma(vol: &mut Volatility, now: DateTime<Utc>) -> Result<Decimal, String> {
    vol.sigma(&v("gate"), &cake(), Side::Sell, &v("bsc_dex"), 60, now)
}

/// Контекст направления на записанных данных (σ, доля — заданные; k = 1, T = 120 с).
fn cx(rec: &Rec, d_: Direction, sigma: Decimal, share: Option<Decimal>) -> DirCtx {
    let p = |id: &VenueId, dex: bool| Presence {
        venue: id.clone(),
        ticker: "CAKE".into(),
        network: (!dex).then(|| "BEP20".into()),
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info: (!dex).then(|| rec.net.clone()),
        via: None,
    };
    let dex_a = d_.buy_on.0 == "bsc_dex";
    let dex_b = d_.sell_on.0 == "bsc_dex";
    DirCtx {
        a: p(&d_.buy_on, dex_a),
        b: p(&d_.sell_on, dex_b),
        dir: d_,
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        cex: vec![],
        sigma,
        transit_s: d("120"),
        k: d("1"),
        h: d("1"),
        closure_share: share,
        return_per_trade: false,
        pair_a: (!dex_a).then(|| rec.pair.clone()),
        pair_b: (!dex_b).then(|| rec.pair.clone()),
        usdt_b: None,
        transfer_tax: (!dex_a).then_some(Decimal::ZERO),
    }
}

/// Источник DEX → BingX (см. шапку): покупка в записанном пуле, продажа по снимку стакана BingX, затраты — из плана.
struct PoolToBook<'a> {
    rec: &'a Rec,
    sell: FrozenBook,
    plan: PlanCosts,
}

impl<'a> PoolToBook<'a> {
    /// Источник по записям rec. Ошибок нет.
    fn new(rec: &'a Rec) -> Self {
        let b = book(rec, "bingx");
        PoolToBook {
            rec,
            sell: FrozenBook {
                venue: b.venue.clone(),
                asset: cake(),
                side: Side::Sell,
                fee: BookFee::in_received(Decimal::ZERO, Side::Sell, "CAKE", "USDT"),
                taken_at: b.received_at,
                book: b,
            },
            plan: PlanCosts::new("CAKE", &[], Decimal::ZERO),
        }
    }
}

impl Source for PoolToBook<'_> {
    /// Покупка — обмен в пуле на точный выход qty; продажа — снимок стакана BingX. Ошибки: TooLarge — нет пути.
    async fn quote(&self, d_: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        if sell {
            return Ok(self.sell.price(qty));
        }
        let out = to_units(qty, 18).unwrap();
        let s = swap(&self.rec.ctx, &self.rec.pool, USDT, false, out)
            .map_err(|e| classify("bsc_dex", &e))?;
        Ok(Quote {
            venue: d_.buy_on.clone(),
            asset: cake(),
            side: Side::Buy,
            qty,
            fillable_qty: qty,
            avg_price: to_decimal(s.amount_in, 18).unwrap() / qty,
            costs: vec![],
            route: None,
            signal_route: None,
            pick: None,
            at: Utc::now(),
        })
    }

    /// DEX — средняя (пары нет); BingX — самый глубокий нужный уровень. Ошибки: Stop — глубины нет.
    fn worst(
        &self,
        _d: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        if !sell {
            return Ok(avg);
        }
        deepest(&self.sell.book, Side::Sell, qty).ok_or_else(|| Fail::Stop("глубины нет".into()))
    }

    /// Затраты «перевод» плана (в записях их нет). Ошибок нет.
    async fn withdraw(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        Ok(self.plan.withdraw(cx, amount))
    }

    /// Затраты «возврат USDT» плана (в записях их нет). Ошибок нет.
    async fn ret(&self, _cx: &DirCtx, _amount: Decimal) -> Result<Vec<Cost>, Fail> {
        Ok(self.plan.ret())
    }

    /// Курсы из плана (core/plan_costs.rs). Ошибки: как у PlanCosts::rates.
    async fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        coin_price: Decimal,
        must: &[Cost],
    ) -> Result<Rates, Fail> {
        self.plan.rates(cx, coin, coin_price, must)
    }
}

fn book(rec: &Rec, venue: &str) -> OrderBook {
    OrderBook {
        venue: v(venue),
        asset: cake(),
        bids: rec.bids.clone(),
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: Utc::now(),
    }
}

/// Источник: покупка — стакан BingX; продажа — DEX из плана (sell None) или стакан BingX под id B.
fn replay(rec: &Rec, d_: &Direction, cex_b: bool) -> Replay {
    let plan =
        Plan::parse(&serde_json::to_value(size_common::calc::calc(rec, d("10"))).unwrap()).unwrap();
    let b = cex_b.then(|| book(rec, &d_.sell_on.0));
    Replay::new(d_, &plan, book(rec, "bingx"), b, ["USDT", "USDT"]).unwrap()
}

/// Запас плана (строка PriceMoveReserve).
fn reserve_of(c: &Calc) -> Decimal {
    c.costs
        .iter()
        .find(|l| l.kind == CostKind::PriceMoveReserve)
        .unwrap()
        .amount
}

/// Запас до решения 240: k·σ·√T·выручка.
fn base(cx: &DirCtx, c: &Calc) -> Decimal {
    cx.k * cx.sigma * cx.transit_s.sqrt().unwrap() * c.proceeds
}

/// JSON расчёта без моментов времени (at — Utc::now при расчёте и у котировок).
fn body(c: &Calc) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("at");
                m.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(c).unwrap();
    strip(&mut v);
    v
}

#[tokio::test]
async fn sigma_settings() {
    let (_core, s) = core("sigma").await;
    set(&s, "profit.sigma_window_min", Scope::Global, json!(60)).await;
    let frames = gate_bids();
    let now = frames.last().unwrap().0;
    let gd = dir("bsc_dex", "gate");
    // 1) Без настроек — σ та же, что раньше.
    let mut vol = series(&frames);
    let was = old_sigma(&mut vol, now).unwrap();
    assert!(was > Decimal::ZERO);
    assert_eq!(reserve::sigma_now(&mut vol, &s, &gd, now).unwrap(), was);
    assert_eq!(reserve::closure_share(&s, &gd, None).unwrap(), None);
    // 2) Граница выше σ ряда — σ = граница; ниже — σ ряда; граница другой площадки не действует.
    let gate = Scope::Venue("gate".into());
    set(
        &s,
        "profit.sigma_floor_per_s",
        Scope::Venue("bingx".into()),
        json!((was * d("3")).to_string()),
    )
    .await;
    assert_eq!(reserve::sigma_now(&mut vol, &s, &gd, now).unwrap(), was);
    set(
        &s,
        "profit.sigma_floor_per_s",
        gate.clone(),
        json!((was * d("2")).to_string()),
    )
    .await;
    assert_eq!(
        reserve::sigma_now(&mut vol, &s, &gd, now).unwrap(),
        was * d("2")
    );
    set(
        &s,
        "profit.sigma_floor_per_s",
        gate.clone(),
        json!((was / d("2")).to_string()),
    )
    .await;
    assert_eq!(reserve::sigma_now(&mut vol, &s, &gd, now).unwrap(), was);
    unset(&s, "profit.sigma_floor_per_s", gate.clone()).await;
    // 3) Ненулевых изменений в окне — столько, сколько раз бид менялся в записи (кадр с тем же временем — не замер).
    let mut changes = 0u64;
    let mut prev = frames[0];
    for f in &frames[1..] {
        if f.0 > prev.0 {
            changes += u64::from(f.1 != prev.1);
            prev = *f;
        }
    }
    assert!(changes > 1);
    set(
        &s,
        "profit.sigma_min_changes",
        Scope::Global,
        json!(changes),
    )
    .await;
    assert_eq!(reserve::sigma_now(&mut vol, &s, &gd, now).unwrap(), was);
    set(
        &s,
        "profit.sigma_min_changes",
        Scope::Global,
        json!(changes + 1),
    )
    .await;
    let e = reserve::sigma_now(&mut vol, &s, &gd, now).unwrap_err();
    assert!(
        e.contains("не измерена") && e.contains("profit.sigma_min_changes"),
        "{e}"
    );
    assert!(
        e.contains(&format!("цены {changes}, нужно не меньше {}", changes + 1)),
        "{e}"
    );
    // Тихий хвост записи (бид не менялся): σ = 0. Без минимума — измерена (запас 0, как раньше); минимум 1 — нет.
    let last = frames.last().unwrap().1;
    let from = frames.iter().rposition(|f| f.1 != last).unwrap() + 1;
    let quiet = &frames[from..];
    assert!(quiet.len() >= 2, "в записи есть неподвижный хвост");
    let mut qv = series(quiet);
    unset(&s, "profit.sigma_min_changes", Scope::Global).await;
    assert_eq!(old_sigma(&mut qv, now).unwrap(), Decimal::ZERO);
    assert_eq!(
        reserve::sigma_now(&mut qv, &s, &gd, now).unwrap(),
        Decimal::ZERO
    );
    set(&s, "profit.sigma_min_changes", Scope::Global, json!(1)).await;
    let e = reserve::sigma_now(&mut qv, &s, &gd, now).unwrap_err();
    assert!(e.contains("цены 0, нужно не меньше 1"), "{e}");
}

#[tokio::test]
async fn reserve_without_settings_and_floor() {
    let (core, s) = core("base").await;
    let rec = Rec::load("reserve-base").await;
    let dd = dir("bingx", "bsc_dex");
    let src = replay(&rec, &dd, false);
    let sigma = d("0.00002");
    // Без настроек: доли нет, запас k·σ·√T·выручка, расчёт побайтно как у контекста без доли.
    let share = reserve::closure_share(&s, &dd, None).unwrap();
    assert_eq!(share, None);
    let c0 = cx(&rec, dd.clone(), sigma, share);
    let r0 = costs::evaluate_with(&c0, d("10"), &src).await.unwrap();
    assert_eq!(reserve_of(&r0), base(&c0, &r0));
    let again = costs::evaluate_with(&cx(&rec, dd.clone(), sigma, None), d("10"), &src)
        .await
        .unwrap();
    assert_eq!(body(&r0), body(&again));
    assert_eq!(r0.profit_usd, r0.proceeds - r0.invested - r0.costs_usdt);
    // Граница σ (sigma_now — тест sigma_settings; в расчёт приходит σ контекста = max(σ ряда, граница)): запас растёт
    // ровно до k·граница·√T·выручка.
    let floor = sigma * d("5");
    let c1 = cx(&rec, dd.clone(), sigma.max(floor), None);
    let r1 = costs::evaluate_with(&c1, d("10"), &src).await.unwrap();
    assert_eq!(
        reserve_of(&r1),
        c1.k * floor * c1.transit_s.sqrt().unwrap() * r1.proceeds
    );
    assert_eq!(
        r1.profit_usd,
        r0.profit_usd - (reserve_of(&r1) - reserve_of(&r0))
    );
    drop(core);
}

#[tokio::test]
async fn closure_share_reserve() {
    let (_core, s) = core("closure").await;
    let rec = Rec::load("reserve-closure").await;
    let dd = dir("bsc_dex", "bingx");
    let src = PoolToBook::new(&rec);
    let sigma = d("0.00002");
    let r0 = costs::evaluate_with(&cx(&rec, dd.clone(), sigma, None), d("10"), &src)
        .await
        .unwrap();
    assert_eq!(
        reserve_of(&r0),
        base(&cx(&rec, dd.clone(), sigma, None), &r0)
    );
    let spread = (r0.sell_avg - r0.buy_avg) / r0.buy_avg;
    assert!(
        spread > Decimal::ZERO,
        "на записи стакан BingX дороже пула: {spread}"
    );
    // Доля у другой площадки-получателя (bsc_dex — направления биржа → DEX) — не действует.
    set(
        &s,
        "profit.closure_share",
        Scope::Venue("bsc_dex".into()),
        json!("0.5"),
    )
    .await;
    assert_eq!(reserve::closure_share(&s, &dd, None).unwrap(), None);
    // Доля у BingX (получатель): запас = доля · спред · выручка (больше k·σ·√T), прибыль меньше ровно на разницу запасов.
    set(
        &s,
        "profit.closure_share",
        Scope::Venue("bingx".into()),
        json!("0.5"),
    )
    .await;
    let share = reserve::closure_share(&s, &dd, None).unwrap();
    assert_eq!(share, Some(d("0.5")));
    let c1 = cx(&rec, dd.clone(), sigma, share);
    let r1 = costs::evaluate_with(&c1, d("10"), &src).await.unwrap();
    let want = d("0.5") * spread * r1.proceeds;
    assert!(want > base(&c1, &r1));
    assert_eq!(reserve_of(&r1), want);
    assert_eq!(r1.profit_usd, r0.profit_usd - (want - reserve_of(&r0)));
    assert_eq!((r1.proceeds, r1.invested), (r0.proceeds, r0.invested));
    // Маленькая доля — k·σ·√T больше, запас и весь расчёт прежние.
    let tiny = cx(&rec, dd.clone(), sigma, Some(d("0.0000001")));
    let r2 = costs::evaluate_with(&tiny, d("10"), &src).await.unwrap();
    assert_eq!(body(&r2), body(&r0));
    // Биржа ↔ биржа на одном стакане (продажа по бидам ниже асков) — спред ≤ 0: доля не действует.
    set(
        &s,
        "profit.closure_share",
        Scope::Venue("mexc".into()),
        json!("0.5"),
    )
    .await;
    let cc = dir("bingx", "mexc");
    let cs = replay(&rec, &cc, true);
    let share = reserve::closure_share(&s, &cc, None).unwrap();
    assert_eq!(share, Some(d("0.5")));
    let with = costs::evaluate_with(&cx(&rec, cc.clone(), sigma, share), d("10"), &cs)
        .await
        .unwrap();
    let without = costs::evaluate_with(&cx(&rec, cc.clone(), sigma, None), d("10"), &cs)
        .await
        .unwrap();
    assert!(with.sell_avg <= with.buy_avg);
    assert_eq!(body(&with), body(&without));
}

#[tokio::test]
async fn pinned_at_entry() {
    let (_core, s) = core("pinned").await;
    set(&s, "profit.sigma_window_min", Scope::Global, json!(60)).await;
    let frames = gate_bids();
    let now = frames.last().unwrap().0;
    let gd = dir("bsc_dex", "gate");
    let gate = Scope::Venue("gate".into());
    let mut vol = series(&frames);
    let was = old_sigma(&mut vol, now).unwrap();
    set(&s, "profit.closure_share", gate.clone(), json!("0.39")).await;
    set(
        &s,
        "profit.sigma_floor_per_s",
        gate.clone(),
        json!((was * d("2")).to_string()),
    )
    .await;
    // Копия настроек на момент входа (как core/frozen.rs take → DirCtx::build_decided).
    let pinned = s.pinned();
    // После входа Тигран меняет все три.
    set(&s, "profit.closure_share", gate.clone(), json!("0.9")).await;
    set(
        &s,
        "profit.sigma_floor_per_s",
        gate.clone(),
        json!((was * d("4")).to_string()),
    )
    .await;
    set(
        &s,
        "profit.sigma_min_changes",
        Scope::Global,
        json!(1_000_000),
    )
    .await;
    assert_eq!(
        reserve::closure_share(&pinned, &gd, None).unwrap(),
        Some(d("0.39"))
    );
    assert_eq!(
        reserve::sigma_now(&mut vol, &pinned, &gd, now).unwrap(),
        was * d("2")
    );
    // Живые — уже новые.
    assert_eq!(
        reserve::closure_share(&s, &gd, None).unwrap(),
        Some(d("0.9"))
    );
    assert!(reserve::sigma_now(&mut vol, &s, &gd, now).is_err());
}
