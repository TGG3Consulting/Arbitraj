//! Файл: tests/sieve_common/mod.rs
//! Что делает: общее для тестов отсева по доказанной границе прибыли (решения 372–377): ядро без площадок, контекст
//! направления, снимки стакана BingX CAKE-USDT и источник эталона (costs.rs Source: биржа — снимок Live, DEX — обмен в
//! записанном пуле CAKE/USDT V2 адаптером) на записанных данных tests/size_common; сеть BEP20 USDT у BingX (возврат USDT)
//! — из той же записи config_getall.json; проверка «граница ≥ полного расчёта» на плотной сетке объёмов.
//! Параметры сценария (σ, k, ставка тейкера, доля/фикс вывода, налог перевода, факты перевода) задаёт тест — в записях
//! их нет; рынок (стакан, пул, правила пары, сети) — только из записей.
//! Вынесено из tests/core_sieve_t162.rs (ревью t162, Н5).
//! Кто вызывает: tests/core_sieve_t162.rs, tests/core_sieve_inputs_t162.rs.
//! От чего зависит: arb::core::{bound, bound_in, costs, dirctx, frozen}, arb::venues::bsc_dex, tests/size_common.
#![allow(dead_code)]

use std::sync::Arc;

use arb::core::Core;
use arb::core::bound::Bound;
use arb::core::bound_in::{SideIn, assemble};
use arb::core::convert::Rates;
use arb::core::costs::{self, Live, Source};
use arb::core::costs_quote::classify;
use arb::core::deps::Deps;
use arb::core::dirctx::DirCtx;
use arb::core::frozen::Frozen;
use arb::core::registry::Presence;
use arb::core::size::Fail;
use arb::core::types::{Asset, CoinNetworkInfo, Cost, Direction, OrderBook, Quote, Side, VenueId};
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::gateway::rate_bound::DexBound;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::bound::rho_raw;
use arb::venues::bsc_dex::quote::pool_quote::swap;
use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::broadcast;

use crate::size_common::{CAKE, Rec, USDT, d, json_file};

/// Направления проверки.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Way {
    CexDex,
    DexCex,
    CexCex,
}

pub fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

pub fn dir(w: Way) -> Direction {
    let (a, b) = match w {
        Way::CexDex => ("bingx", "bsc_dex"),
        Way::DexCex => ("bsc_dex", "bingx"),
        Way::CexCex => ("bingx", "bingx"),
    };
    Direction {
        asset: cake(),
        buy_on: VenueId(a.into()),
        sell_on: VenueId(b.into()),
    }
}

/// Присутствие монеты на площадке: биржа — с сетью BEP20 `net`, DEX — без.
pub fn presence(net: &CoinNetworkInfo, cex: bool) -> Presence {
    Presence {
        venue: VenueId(if cex { "bingx" } else { "bsc_dex" }.into()),
        ticker: "CAKE".into(),
        network: Some("BEP20".into()),
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info: cex.then(|| net.clone()),
        via: None,
    }
}

/// Контекст направления на записанных данных; σ, T, k — параметры сценария, h = 0,7 (как tests/core_size_budget_early.rs).
/// Сеть вывода A — `net` (запись или запись с изменённой комиссией), налог перевода — `tax` (у биржи A).
pub fn cx_with(
    rec: &Rec,
    w: Way,
    sigma: Decimal,
    k: Decimal,
    net: &CoinNetworkInfo,
    tax: Decimal,
) -> DirCtx {
    let (a_cex, b_cex) = (w != Way::DexCex, w != Way::CexDex);
    DirCtx {
        dir: dir(w),
        a: presence(net, a_cex),
        b: presence(net, b_cex),
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        cex: vec![],
        sigma,
        transit_s: d("20"),
        k,
        h: d("0.7"),
        closure_share: None,
        sigma_check: None,
        return_per_trade: false,
        pair_a: a_cex.then(|| rec.pair.clone()),
        pair_b: b_cex.then(|| rec.pair.clone()),
        usdt_b: None,
        transfer_tax: a_cex.then_some(tax),
        sell_max: None,
    }
}

/// Контекст направления с сетью записи и налогом 0.
pub fn cx(rec: &Rec, w: Way, sigma: Decimal, k: Decimal) -> DirCtx {
    cx_with(rec, w, sigma, k, &rec.net, Decimal::ZERO)
}

/// Сеть BEP20 USDT у BingX из записи config_getall.json (возврат USDT в каждой сделке).
pub fn usdt_net() -> CoinNetworkInfo {
    let cfg = json_file("tests/data/bingx_2026-10-24/config_getall.json");
    let coin = cfg["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["coin"] == "USDT")
        .unwrap();
    let n = coin["networkList"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["network"] == "BEP20")
        .unwrap();
    let num = |k: &str| n[k].as_str().map(d);
    CoinNetworkInfo {
        venue: VenueId("bingx".into()),
        asset: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        network: "BEP20".into(),
        deposit_enabled: true,
        withdraw_enabled: true,
        withdraw_fee: num("withdrawFee"),
        withdraw_fee_pct: None,
        withdraw_min: num("withdrawMin"),
        withdraw_max: num("withdrawMax"),
        deposit_min: num("depositMin"),
        deposit_confirmations: None,
        withdraw_precision: n["withdrawPrecision"].as_u64().map(|p| p as u32),
        withdraw_step: None,
        updated_at: Utc::now(),
    }
}

/// Снимок стакана BingX стороны с планом комиссии `fee`.
pub fn frozen_with(rec: &Rec, side: Side, fee: BookFee) -> FrozenBook {
    let book = OrderBook {
        venue: VenueId("bingx".into()),
        asset: cake(),
        bids: rec.bids.clone(),
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: Utc::now(),
    };
    FrozenBook {
        venue: book.venue.clone(),
        asset: cake(),
        side,
        book: book.clone(),
        fee,
        taken_at: book.received_at,
    }
}

/// Снимок стакана BingX стороны (комиссия taker 0,1 % «в получаемой монете», как адаптер BingX — BookFee::in_received).
pub fn frozen(rec: &Rec, side: Side) -> FrozenBook {
    frozen_with(
        rec,
        side,
        BookFee::in_received(d("0.001"), side, "CAKE", "USDT"),
    )
}

/// Источник эталона: биржа — снимок (Live), DEX — обмен в записанном пуле (адаптер DEX).
pub struct Src<'a> {
    pub live: Live<'a>,
    pub rec: &'a Rec,
    pub dex_a: bool,
    pub dex_b: bool,
}

impl Src<'_> {
    /// Котировка DEX: продажа — точный вход, покупка — точный выход (маршрут покупки «с конца»).
    fn dex(&self, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        let units = to_units(qty, 18).ok_or(Fail::TooSmall("объём".into()))?;
        let (tin, exact_in) = if sell { (CAKE, true) } else { (USDT, false) };
        let s = swap(&self.rec.ctx, &self.rec.pool, tin, exact_in, units)
            .map_err(|e| classify("bsc_dex", &e))?;
        let usdt = to_decimal(if sell { s.amount_out } else { s.amount_in }, 18).unwrap();
        Ok(Quote {
            venue: VenueId("bsc_dex".into()),
            asset: cake(),
            side: if sell { Side::Sell } else { Side::Buy },
            qty,
            fillable_qty: qty,
            avg_price: usdt / qty,
            costs: vec![],
            route: None,
            signal_route: None,
            pick: None,
            at: Utc::now(),
        })
    }
}

impl Source for Src<'_> {
    async fn quote(&self, d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        if (sell && self.dex_b) || (!sell && self.dex_a) {
            return self.dex(sell, qty);
        }
        self.live.quote(d, sell, qty).await
    }

    fn worst(
        &self,
        d: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        if (sell && self.dex_b) || (!sell && self.dex_a) {
            return Ok(avg);
        }
        self.live.worst(d, sell, qty, avg)
    }

    async fn withdraw(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        // DEX на A: газ перевода (≥ 0) в записи нет — не вычитается (эталон только выше); налог перевода CAKE 0.
        if self.dex_a {
            return Ok(vec![]);
        }
        self.live.withdraw(cx, amount).await
    }

    async fn ret(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        self.live.ret(cx, amount).await
    }

    async fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        price: Decimal,
        must: &[Cost],
    ) -> Result<Rates, Fail> {
        self.live.rates(cx, coin, price, must).await
    }
}

/// Настоящее ядро без площадок (как tests/core_size_budget_early.rs).
pub async fn core(dir: &std::path::Path) -> Core {
    let db = Db::open(&dir.join("arb.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let vault = arb::secrets::Vault::locked(dir.join("secrets.enc"));
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let deps = Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: dir.to_path_buf(),
    };
    Core::start(deps).await
}

/// Верх DEX по записанному пулу — функция адаптера (decimals CAKE и USDT в BSC — 18, множитель 1).
pub fn dex_bound(rec: &Rec, sell: bool) -> DexBound {
    let (from, to) = if sell { (CAKE, USDT) } else { (USDT, CAKE) };
    let rho =
        rho_raw(&rec.ctx, std::slice::from_ref(&rec.pool), from, to, &[], 2).expect("верх пула V2");
    DexBound {
        rho,
        pools: 1,
        hops: 2,
    }
}

/// Сетка объёмов: кратные шагу пары до глубины стакана/пула + границы уровней asks (± шаг).
pub fn grid(rec: &Rec, n: u32) -> Vec<Decimal> {
    let step = rec.pair.step_size;
    let depth: Decimal = rec.asks.iter().map(|l| l.qty).sum();
    let mut out: Vec<Decimal> = (1..=n)
        .map(|i| ((depth * Decimal::from(i) / Decimal::from(n)) / step).floor() * step)
        .collect();
    let mut acc = Decimal::ZERO;
    for l in &rec.asks {
        acc += l.qty;
        for q in [acc - step, acc, acc + step] {
            out.push((q / step).floor() * step);
        }
    }
    out.retain(|q| *q > Decimal::ZERO);
    out.sort();
    out.dedup();
    out
}

/// Снимки сторон для сценария.
pub struct Books {
    pub a: FrozenBook,
    pub b: FrozenBook,
}

/// Что делает: граница через настоящий assemble (стаканы снимков, ставка — по котировке снимка, верх DEX — rho_raw) и
/// эталон на сетке: U(q) ≥ прибыли evaluate_with в каждой допустимой точке. Принимает: ядро, запись, направление,
/// контекст, снимки, факт перевода для границы. Возвращает: (граница, точки (объём, прибыль, %)). Ошибки: паника теста —
/// нарушение или граница не собирается.
pub async fn covers(
    core: &Core,
    rec: &Rec,
    w: Way,
    c: &DirCtx,
    books: &Books,
    seen: Option<&arb::core::bound_in::Seen>,
) -> (Bound, Vec<(Decimal, Decimal, Decimal)>) {
    let fz = Frozen {
        a: Arc::new(books.a.clone()),
        b: Arc::new(books.b.clone()),
        at: Utc::now(),
        decided: None,
    };
    let src = Src {
        live: Live {
            core,
            src: Some(&fz),
        },
        rec,
        dex_a: w == Way::DexCex,
        dex_b: w == Way::CexDex,
    };
    let (sa, sb) = (books.a.price(d("1")), books.b.price(d("1")));
    let (da, db) = (dex_bound(rec, false), dex_bound(rec, true));
    let a = if src.dex_a {
        SideIn::Dex(&da)
    } else {
        SideIn::Book {
            book: &books.a.book,
            sample: &sa,
            ticker: "CAKE",
        }
    };
    let b = if src.dex_b {
        SideIn::Dex(&db)
    } else {
        SideIn::Book {
            book: &books.b.book,
            sample: &sb,
            ticker: "CAKE",
        }
    };
    let bound = assemble(c, &a, &b, seen).expect("граница собирается");
    let mut pts = Vec::new();
    for q in grid(rec, 600) {
        let Ok(calc) = costs::evaluate_with(c, q, &src).await else {
            continue;
        };
        let (u, _) = bound.u_at(q).expect("U(q)");
        assert!(
            u >= calc.profit_usd,
            "{w:?} q {q}: граница {u} < прибыли {}",
            calc.profit_usd
        );
        pts.push((q, calc.profit_usd, calc.profit_pct));
    }
    (bound, pts)
}
