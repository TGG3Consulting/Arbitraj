//! Файл: tests/core_size_budget_early.rs
//! Что проверяет: решение 246 — подбор объёма с бюджетом у кривой (costs.rs evaluate_capped) выбирает тот же объём с
//! той же прибылью, что без него (costs.rs evaluate_with), а котировок стороны B (DEX) считает меньше: объём сверх
//! бюджета отбрасывается сразу после стороны A.
//! Данные (tests/size_common, записанные): стакан BingX CAKE-USDT@depth20 (покупка на A — снимок core/frozen.rs, как
//! расчёт по сделке, решение 217), правила пары и сеть BEP20 CAKE у BingX, пул CAKE/USDT PancakeSwap V2 с резервами
//! getReserves (продажа на B — обмен адаптера DEX quote/pool_quote.rs). Записанных пулов монет живого случая 09.11
//! (PIT, BULLA, CHEEMS …) в tests/data нет — направление то же (биржа → bsc_dex), монета CAKE.
//! Ставки taker аккаунта в записях нет — 0,1 % (как tests/uncapped_recompute.rs); h = 0,7 — значение с сервера 09.11
//! (docs/notes/price-move-vs-reserve-2026-11-09.md), поэтому на объём две котировки B (глубина sell_qty / h и sell_qty).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{costs, size_search, size_curve, frozen}, arb::venues::bsc_dex::quote::pool_quote,
//! tests/size_common.

mod size_common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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
use arb::core::types::{Asset, Cost, Direction, OrderBook, Quote, Side, VenueId};
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use size_common::curve::Dir;
use size_common::{CAKE, Rec, USDT, d};
use tokio::sync::broadcast;

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn dir() -> Direction {
    Direction {
        asset: cake(),
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    }
}

/// Контекст направления bingx → bsc_dex на записанных данных (правила пары и сеть BingX; у DEX пары и сети нет).
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
        dir: dir(),
        a: p("bingx", Some(rec.net.clone())),
        b: p("bsc_dex", None),
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
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

/// Снимок стакана BingX (покупка — комиссия taker 0,1 % в монете, как адаптер BingX).
fn frozen_buy(rec: &Rec) -> FrozenBook {
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

/// Источник: A — снимок стакана BingX (Live), B — обмен в записанном пуле (адаптер DEX); считает котировки B.
struct Src<'a> {
    live: Live<'a>,
    rec: &'a Rec,
    dex: AtomicU32,
}

impl Source for Src<'_> {
    async fn quote(&self, d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        if !sell {
            return self.live.quote(d, false, qty).await;
        }
        self.dex.fetch_add(1, Ordering::Relaxed);
        let s = swap(
            &self.rec.ctx,
            &self.rec.pool,
            CAKE,
            true,
            to_units(qty, 18).unwrap(),
        )
        .map_err(|e| classify("bsc_dex", &e))?;
        let out = to_decimal(s.amount_out, 18).unwrap();
        Ok(Quote {
            venue: d.sell_on.clone(),
            asset: cake(),
            side: Side::Sell,
            qty,
            fillable_qty: qty,
            avg_price: out / qty,
            costs: vec![],
            route: None,
            signal_route: None,
            pick: None,
            at: Utc::now(),
        })
    }

    fn worst(
        &self,
        d: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        // У DEX пары нет — пределов ордера нет (order_limits без пары); худшая цена на выбор не влияет.
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

/// Кривая подбора над источником: бюджет у кривой (решение 246) или нет (как до него).
struct C<'a> {
    cx: &'a DirCtx,
    src: &'a Src<'a>,
    budget: Option<Decimal>,
}

#[async_trait]
impl Curve for C<'_> {
    type Data = Calc;

    async fn at(&mut self, qty: Decimal) -> Result<Point<Calc>, Fail> {
        let c = costs::evaluate_capped(self.cx, qty, self.src, self.budget).await?;
        Ok(Point {
            qty,
            invested: c.spend_max,
            arrive: c.arrive,
            profit: c.profit_usd,
            data: c,
        })
    }
}

/// Настоящее ядро без площадок (как tests/uncapped_recompute.rs).
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
async fn budget_on_curve_same_choice_fewer_dex_quotes() {
    let dirp = std::env::temp_dir().join(format!("arb_budget_early_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dirp);
    std::fs::create_dir_all(&dirp).unwrap();
    let core = core(&dirp).await;
    let rec = Rec::load("budget-early").await;
    let c = cx(&rec);
    let fz = Frozen {
        a: Arc::new(frozen_buy(&rec)),
        b: Arc::new(frozen_buy(&rec)),
        at: Utc::now(),
        decided: None,
    };
    let mut tried = 0;
    for budget in [d("50"), d("300"), d("1000")] {
        let p = size_common::params(&rec, Dir::BingxToDex, budget);
        let run = |cap: Option<Decimal>| {
            let src = Src {
                live: Live {
                    core: &core,
                    src: Some(&fz),
                },
                rec: &rec,
                dex: AtomicU32::new(0),
            };
            let p = p.clone();
            let c = &c;
            async move {
                let mut curve = C {
                    cx: c,
                    src: &src,
                    budget: cap,
                };
                let r = search(&mut curve, &p).await;
                (r, src.dex.load(Ordering::Relaxed))
            }
        };
        let (old, n_old) = run(None).await;
        let (new, n_new) = run(Some(budget)).await;
        let (old, new) = (old.unwrap(), new.unwrap());
        println!(
            "бюджет {budget}: объём {} прибыль {} (расчётов {}), котировок DEX: было {n_old}, стало {n_new}",
            new.choice.qty, new.choice.profit, new.choice.evals
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
        assert!(n_new <= n_old);
        if n_new < n_old {
            tried += 1;
        }
    }
    assert!(
        tried > 0,
        "ни на одном бюджете ранняя проверка не сработала"
    );
    let _ = std::fs::remove_dir_all(&dirp);
}
