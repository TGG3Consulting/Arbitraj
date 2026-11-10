//! Файл: tests/bsc_dex_quote_fills.rs
//! Что проверяет: решение 250 — проверка доли глубины h (core/costs.rs) через исполнимость без полной котировки
//! (bsc_dex quote/fills.rs fast):
//! 1) точность: на пулах РЕАЛЬНОЙ записи CAKE 2026-10-29 (tests/route_common: CAKE-USDT PancakeSwap V3 и Uniswap V3,
//!    пути через WBNB/BTCB; блок 130521896) на сетке объёмов от 1 до 10⁷ CAKE: везде, где быстрый ответ «исполнит
//!    целиком», полная котировка выбора маршрута (quote/routing.rs run — классика и граф со сплитом) тоже даёт
//!    fillable_qty = qty; в обоих режимах dex.router_mode (classic, graph); быстрый ответ встречается и в «да», и в «нет»;
//! 2) подбор объёма (size_search) направления bingx → bsc_dex: A — записанный стакан BingX CAKE (tests/size_common),
//!    B — та же запись пулов CAKE, продажа — полная котировка адаптера (routing::run), h = 0,7: с быстрым ответом на
//!    доле глубины выбранный объём, прибыль, границы, точность границы, число расчётов, пики и затраты — ТЕ ЖЕ, что с
//!    полной котировкой на доле глубины, а полных котировок DEX меньше (числа — в выводе теста).
//!
//! Цену газа узла тест не спрашивает: выбор маршрута (routing::run) получает её параметром, а быстрый ответ — без цены
//! газа (в Quoting::fills он стоит после проверок свежести, как у quote).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{fills, routing}, arb::core::{costs, size_search}, tests/route_common,
//! tests/size_common.

mod route_common;
mod size_common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use alloy_primitives::U256;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::Core;
use arb::core::convert::Rates;
use arb::core::costs::{self, Calc, Live, Source};
use arb::core::costs_quote::classify;
use arb::core::deps::Deps;
use arb::core::dirctx::DirCtx;
use arb::core::frozen::Frozen;
use arb::core::registry::Presence;
use arb::core::size::{Curve, Fail, Point};
use arb::core::size_search::search;
use arb::core::types::{Asset, Cost, Direction, OrderBook, Quote, Side, TokenTaxes, VenueId};
use arb::gateway::errors::GatewayResult;
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::{Quoting, fills, routing};
use size_common::curve::Dir;
use size_common::{Rec, d};
use tokio::sync::broadcast;

const BLOCK: u64 = 130_521_896;
const GAS: u64 = 1_000_000_000;

/// CAKE записи пулов (route_common).
fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(route_common::CAKE),
    }
}

async fn set(p: &Parts, name: &str, v: serde_json::Value) {
    p.ctx
        .settings
        .set(name, Scope::Venue("bsc_dex".into()), Some(v), "тест")
        .await
        .unwrap();
}

/// Запись пулов CAKE, свежая голова, прямой режим, режим роутера `router`. Ошибок нет.
async fn scene(router: &str) -> Parts {
    let p = route_common::parts().await;
    set(&p, "feed.stale_after_ms", json!(600_000)).await;
    set(&p, "dex.swap_mode", json!("direct")).await;
    set(&p, "dex.router_mode", json!(router)).await;
    p.heads.observe(BLOCK);
    p
}

/// Полная котировка выбора маршрута (как Quoting::priced до газа разрешений и чаевых): классика и, в graph-режиме,
/// граф со сплитом. Принимает: котировку, сторону, количество. Возвращает: котировку или ошибку адаптера.
async fn full(q: &Quoting, side: Side, qty: Decimal) -> GatewayResult<Quote> {
    let ctx = &q.ctx;
    let mids = q.mids.active(ctx).into_iter().map(|m| (m, 18u8)).collect();
    let input = routing::Input {
        asset: cake(),
        side,
        qty,
        coin: route_common::CAKE,
        usdt: route_common::USDT,
        taxes: TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        },
        coin_dec: 18,
        usdt_dec: 18,
        entries: q.reg.by_pair(route_common::CAKE, route_common::USDT),
        mids,
        gas: U256::from(GAS),
        usdt_ticker: "USDT".into(),
        smart: false,
    };
    routing::run(q.clone(), input).await
}

/// Объёмы сетки: 1 … 10⁷ CAKE с шагом ×√10 (≈3.16) и точки вокруг границы исполнимости.
fn grid() -> Vec<Decimal> {
    let mut v = vec![];
    let mut x = Decimal::ONE;
    while x <= Decimal::from(10_000_000u64) {
        v.push(x);
        v.push((x * d("3.16")).round_dp(2));
        x *= Decimal::TEN;
    }
    v
}

#[tokio::test]
async fn fast_answer_never_says_full_when_full_quote_does_not() {
    for router in ["classic", "graph"] {
        let p = scene(router).await;
        let q = p.quoting();
        let (mut yes, mut no) = (0, 0);
        for qty in grid() {
            let f = fills::fast(&q, &cake(), Side::Sell, qty).await.unwrap();
            let r = full(&q, Side::Sell, qty).await;
            let got = r.as_ref().map(|q| q.fillable_qty).ok();
            if f {
                yes += 1;
                assert_eq!(
                    got,
                    Some(qty),
                    "{router}: быстрый ответ «целиком» при {qty}, полная котировка: {:?}",
                    r.map(|q| q.fillable_qty).map_err(|e| e.message)
                );
            } else {
                no += 1;
            }
            // Покупка — всегда полная котировка (быстрого ответа нет).
            assert!(!fills::fast(&q, &cake(), Side::Buy, qty).await.unwrap());
        }
        println!("{router}: быстрый ответ «целиком» {yes}, «нужна полная котировка» {no}");
        assert!(
            yes > 0 && no > 0,
            "{router}: сетка должна пересечь границу исполнимости"
        );
        // Умный режим — быстрого ответа нет.
        set(&p, "dex.swap_mode", json!("smart")).await;
        assert!(
            !fills::fast(&q, &cake(), Side::Sell, Decimal::ONE)
                .await
                .unwrap()
        );
    }
}

/// Источник подбора: A — снимок стакана BingX (Live со снимком), B — котировка адаптера по записи пулов.
struct Src<'a> {
    live: Live<'a>,
    q: Quoting,
    /// true — доля глубины через быстрый ответ (решение 250); false — полной котировкой (как до решения).
    fast: bool,
    full: AtomicU32,
    hits: AtomicU32,
}

impl Src<'_> {
    /// Полная котировка продажи B со счётчиком. Ошибки: Fail по виду ошибки адаптера.
    async fn sell(&self, qty: Decimal) -> Result<Quote, Fail> {
        self.full.fetch_add(1, Ordering::Relaxed);
        let mut q = full(&self.q, Side::Sell, qty)
            .await
            .map_err(|e| classify("bsc_dex", &e))?;
        // Курса BNB у ядра без площадок нет: газ (затрата в BNB вне цены) в тесте не считается — одинаково в обоих
        // прогонах; комиссия пула и прочее «в цене» остаются.
        q.costs.retain(|c| c.in_price);
        Ok(q)
    }
}

impl Source for Src<'_> {
    async fn quote(&self, d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        if !sell {
            return self.live.quote(d, false, qty).await;
        }
        self.sell(qty).await
    }

    async fn fillable(&self, d: &Direction, sell: bool, qty: Decimal) -> Result<Decimal, Fail> {
        if sell && self.fast {
            let f = fills::fast(&self.q, &cake(), Side::Sell, qty)
                .await
                .map_err(|e| classify("bsc_dex", &e))?;
            if f {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(qty);
            }
        }
        self.quote(d, sell, qty).await.map(|q| q.fillable_qty)
    }

    fn worst(
        &self,
        d: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        // У DEX пары нет — пределов ордера нет; худшая цена продажи на выбор не влияет.
        if sell {
            return Ok(avg);
        }
        self.live.worst(d, false, qty, avg)
    }

    async fn withdraw(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        self.live.withdraw(cx, amount).await
    }

    async fn ret(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        self.live.ret(cx, amount).await
    }

    async fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        coin_price: Decimal,
        must: &[Cost],
    ) -> Result<Rates, Fail> {
        self.live.rates(cx, coin, coin_price, must).await
    }
}

/// Кривая подбора над источником (бюджет у кривой, как бой — решение 246).
struct C<'a> {
    cx: &'a DirCtx,
    src: &'a Src<'a>,
    budget: Decimal,
}

#[async_trait]
impl Curve for C<'_> {
    type Data = Calc;

    async fn at(&mut self, qty: Decimal) -> Result<Point<Calc>, Fail> {
        let c = costs::evaluate_capped(self.cx, qty, self.src, Some(self.budget)).await?;
        Ok(Point {
            qty,
            invested: c.spend_max,
            arrive: c.arrive,
            profit: c.profit_usd,
            data: c,
        })
    }
}

/// Контекст bingx → bsc_dex на записанных данных (правила пары и сеть BingX, h = 0,7).
fn cx(rec: &Rec) -> DirCtx {
    let p = |v: &str, info| Presence {
        venue: VenueId(v.into()),
        ticker: "CAKE".into(),
        network: Some("BEP20".into()),
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info,
        via: None,
    };
    DirCtx {
        dir: Direction {
            asset: Asset {
                ticker: "CAKE".into(),
                bsc_contract: Some(size_common::CAKE),
            },
            buy_on: VenueId("bingx".into()),
            sell_on: VenueId("bsc_dex".into()),
        },
        a: p("bingx", Some(rec.net.clone())),
        b: p("bsc_dex", None),
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(size_common::USDT),
        },
        cex: vec![],
        sigma: d("0.0005"),
        transit_s: d("20"),
        k: d("1"),
        h: d("0.7"),
        closure_share: None,
        return_per_trade: false,
        pair_a: Some(rec.pair.clone()),
        pair_b: None,
        usdt_b: None,
        transfer_tax: Some(Decimal::ZERO),
    }
}

/// Снимок стакана BingX (покупка — комиссия taker 0,1 % в монете).
fn frozen_buy(rec: &Rec) -> FrozenBook {
    let asset = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(size_common::CAKE),
    };
    let book = OrderBook {
        venue: VenueId("bingx".into()),
        asset: asset.clone(),
        bids: rec.bids.clone(),
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: Utc::now(),
    };
    FrozenBook {
        venue: book.venue.clone(),
        asset: asset.clone(),
        side: Side::Buy,
        book: book.clone(),
        fee: BookFee {
            rate: d("0.001"),
            per_coin: true,
            asset: "CAKE".into(),
            paid_in: None,
        },
        taken_at: book.received_at,
    }
}

/// Настоящее ядро без площадок (как tests/core_size_budget_early.rs).
async fn core(dir: &std::path::Path) -> Core {
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

#[tokio::test]
async fn size_choice_same_with_fewer_full_dex_quotes() {
    let dirp = std::env::temp_dir().join(format!("arb_quote_fills_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dirp);
    std::fs::create_dir_all(&dirp).unwrap();
    let core = core(&dirp).await;
    let rec = Rec::load("quote-fills").await;
    let c = cx(&rec);
    let fz = Frozen {
        a: Arc::new(frozen_buy(&rec)),
        b: Arc::new(frozen_buy(&rec)),
        at: Utc::now(),
        decided: None,
    };
    let mut saved = 0;
    for router in ["classic", "graph"] {
        let p = scene(router).await;
        for budget in [d("50"), d("300"), d("1000"), d("100000")] {
            let params = size_common::params(&rec, Dir::BingxToDex, budget);
            let run = |fast: bool| {
                let src = Src {
                    live: Live {
                        core: &core,
                        src: Some(&fz),
                    },
                    q: p.quoting(),
                    fast,
                    full: AtomicU32::new(0),
                    hits: AtomicU32::new(0),
                };
                let params = params.clone();
                let c = &c;
                async move {
                    let mut curve = C {
                        cx: c,
                        src: &src,
                        budget,
                    };
                    let r = search(&mut curve, &params).await;
                    (
                        r,
                        src.full.load(Ordering::Relaxed),
                        src.hits.load(Ordering::Relaxed),
                    )
                }
            };
            let (old, n_old, _) = run(false).await;
            let (new, n_new, hits) = run(true).await;
            let (old, new) = match (old, new) {
                (Ok(o), Ok(n)) => (o, n),
                (o, n) => {
                    // Отказ подбора — тот же у обоих (вид и текст).
                    assert_eq!(o.err(), n.err(), "{router}, бюджет {budget}");
                    println!("{router}, бюджет {budget}: подбор — отказ, одинаковый");
                    continue;
                }
            };
            println!(
                "{router}, бюджет {budget}: объём {} прибыль {} [{} … {}] расчётов {} пиков {}; полных котировок DEX: \
                 было {n_old}, стало {n_new} (быстрых ответов {hits})",
                new.choice.qty,
                new.choice.profit,
                new.choice.min_qty,
                new.choice.max_qty,
                new.choice.evals,
                new.choice.peaks
            );
            assert_eq!(old.choice.qty, new.choice.qty);
            assert_eq!(old.choice.profit, new.choice.profit);
            assert_eq!(old.choice.min_qty, new.choice.min_qty);
            assert_eq!(old.choice.max_qty, new.choice.max_qty);
            assert_eq!(old.choice.at_budget_profit, new.choice.at_budget_profit);
            assert_eq!(old.choice.max_exact, new.choice.max_exact);
            assert_eq!(old.choice.evals, new.choice.evals);
            assert_eq!(old.choice.peaks, new.choice.peaks);
            assert_eq!(old.choice.capped, new.choice.capped);
            assert_eq!(old.best.data.costs_usdt, new.best.data.costs_usdt);
            assert_eq!(old.best.data.sell_avg, new.best.data.sell_avg);
            assert_eq!(
                n_old,
                n_new + hits,
                "каждый быстрый ответ — минус одна полная котировка"
            );
            if hits > 0 {
                saved += 1;
            }
        }
    }
    assert!(saved > 0, "быстрый ответ не сработал ни разу");
    let _ = std::fs::remove_dir_all(&dirp);
}
