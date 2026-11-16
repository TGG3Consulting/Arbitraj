//! Файл: tests/core_sieve_t162.rs
//! Что проверяет: решения 361–363 — отсев кандидата по доказанной границе прибыли (core/bound.rs, core/bound_in.rs):
//! 1) граница ≥ прибыли ПОЛНОГО расчёта (costs.rs evaluate_with — та же модель, что у бота) на плотной сетке объёмов
//!    (эталон — перебор) по трём направлениям на записанных данных: биржа → DEX (стакан BingX CAKE-USDT → пул CAKE/USDT
//!    PancakeSwap V2), DEX → биржа (покупка в том же пуле точным выходом → bids BingX), биржа → биржа (asks → bids того
//!    же стакана); верх DEX — функция адаптера (quote/bound.rs rho_raw) на том же пуле;
//! 2) тест-сторож: при любом пороге (USDT и %), при котором граница отсекает, НИ ОДНА точка сетки не проходит пороги;
//!    при котором хоть одна точка проходит — граница НЕ отсекает;
//! 3) 18 записанных отказов кандидатов (tests/data/cand_rejects_2026-11-16/cases.json, Calc на объёме записи): по
//!    числам записи (прокси: стаканов и пулов в записи нет — лучшая цена = средняя записи, как в проекте, раздел Б.4)
//!    граница на объёме записи ≥ прибыли записи у всех 18, отсечено 17 из 18, NXT (почти-сделка) — нет.
//!
//! Данные (tests/size_common): стакан BingX CAKE-USDT@depth20, правила пары и сеть BEP20 CAKE у BingX, пул CAKE/USDT V2.
//! Ставки taker в записях нет — 0,1 % (как tests/core_size_budget_early.rs). Затраты эталона, которых нет в записи (газ
//! DEX, перевод с DEX), не вычитаются — прибыль эталона только выше, проверка строже.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{bound, bound_in, costs, dirctx}, arb::venues::bsc_dex::quote::{bound, pool_quote},
//! tests/size_common.

mod size_common;

use std::str::FromStr;
use std::sync::Arc;

use arb::core::Core;
use arb::core::bound::{Arrive, Bound, Buy, Sell};
use arb::core::bound_in::{SideIn, assemble};
use arb::core::convert::Rates;
use arb::core::costs::{self, Live, Source};
use arb::core::costs_quote::classify;
use arb::core::deps::Deps;
use arb::core::dirctx::DirCtx;
use arb::core::frozen::Frozen;
use arb::core::registry::Presence;
use arb::core::size::Fail;
use arb::core::types::{Asset, Cost, Direction, OrderBook, PriceLevel, Quote, Side, VenueId};
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
use serde_json::Value;
use size_common::{CAKE, Rec, USDT, d};
use tokio::sync::broadcast;

/// Направления проверки.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Way {
    CexDex,
    DexCex,
    CexCex,
}

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn dir(w: Way) -> Direction {
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

/// Присутствие монеты на площадке: биржа — с сетью BEP20 из записи, DEX — без.
fn presence(rec: &Rec, cex: bool) -> Presence {
    Presence {
        venue: VenueId(if cex { "bingx" } else { "bsc_dex" }.into()),
        ticker: "CAKE".into(),
        network: Some("BEP20".into()),
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info: cex.then(|| rec.net.clone()),
        via: None,
    }
}

/// Контекст направления на записанных данных; σ, T, k — параметры сценария, h = 0,7 (как tests/core_size_budget_early.rs).
fn cx(rec: &Rec, w: Way, sigma: Decimal, k: Decimal) -> DirCtx {
    let (a_cex, b_cex) = (w != Way::DexCex, w != Way::CexDex);
    DirCtx {
        dir: dir(w),
        a: presence(rec, a_cex),
        b: presence(rec, b_cex),
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
        transfer_tax: a_cex.then_some(Decimal::ZERO),
    }
}

/// Снимок стакана BingX стороны (комиссия taker 0,1 % «в получаемой монете», как адаптер BingX — BookFee::in_received).
fn frozen(rec: &Rec, side: Side) -> FrozenBook {
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
        fee: BookFee::in_received(d("0.001"), side, "CAKE", "USDT"),
        taken_at: book.received_at,
    }
}

/// Источник эталона: биржа — снимок (Live), DEX — обмен в записанном пуле (адаптер DEX).
struct Src<'a> {
    live: Live<'a>,
    rec: &'a Rec,
    dex_a: bool,
    dex_b: bool,
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

/// Верх DEX по записанному пулу — функция адаптера (decimals CAKE и USDT в BSC — 18, множитель 1).
fn dex_bound(rec: &Rec, sell: bool) -> DexBound {
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
fn grid(rec: &Rec, n: u32) -> Vec<Decimal> {
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

/// Один сценарий: эталон на сетке против границы; сторож по набору порогов. Возвращает: (точек, отсечений).
async fn scenario(core: &Core, rec: &Rec, w: Way, sigma: Decimal, k: Decimal) -> (usize, usize) {
    let c = cx(rec, w, sigma, k);
    let (fa, fb) = (frozen(rec, Side::Buy), frozen(rec, Side::Sell));
    let fz = Frozen {
        a: Arc::new(fa.clone()),
        b: Arc::new(fb.clone()),
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
    let (sa, sb) = (fa.price(d("1")), fb.price(d("1")));
    let (da, db) = (dex_bound(rec, false), dex_bound(rec, true));
    let a = if src.dex_a {
        SideIn::Dex(&da)
    } else {
        SideIn::Book {
            book: &fa.book,
            sample: &sa,
            ticker: "CAKE",
        }
    };
    let b = if src.dex_b {
        SideIn::Dex(&db)
    } else {
        SideIn::Book {
            book: &fb.book,
            sample: &sb,
            ticker: "CAKE",
        }
    };
    let bound = assemble(&c, &a, &b, None).expect("граница собирается");
    let mut pts: Vec<(Decimal, Decimal, Decimal)> = Vec::new();
    for q in grid(rec, 600) {
        let Ok(calc) = costs::evaluate_with(&c, q, &src).await else {
            continue;
        };
        let (u, _) = bound.u_at(q).expect("U(q)");
        assert!(
            u >= calc.profit_usd,
            "{w:?} σ {sigma} k {k} q {q}: граница {u} < прибыли {}",
            calc.profit_usd
        );
        pts.push((q, calc.profit_usd, calc.profit_pct));
    }
    assert!(
        pts.len() > 20,
        "{w:?}: допустимых точек сетки мало: {}",
        pts.len()
    );
    let best_p = pts.iter().map(|p| p.1).max().unwrap();
    let mut cuts = 0;
    let usd = [
        best_p - d("1"),
        best_p - d("0.0001"),
        best_p,
        best_p + d("0.0001"),
        best_p + d("1"),
        d("-1000"),
        d("1000"),
    ];
    for t_usd in usd {
        for t_pct in [d("-100"), d("0"), d("0.1"), d("1"), d("5"), d("50")] {
            let best = bound.best(Some(t_pct)).expect("максимумы границы");
            let cut = best.below_usd(t_usd) || best.below_pct();
            let passes = pts.iter().any(|(_, p, pc)| *p >= t_usd && *pc >= t_pct);
            assert!(
                !(cut && passes),
                "{w:?}: отсечено при пороге {t_usd} / {t_pct}%, а точка сетки проходит"
            );
            cuts += usize::from(cut);
        }
    }
    (pts.len(), cuts)
}

/// Граница ≥ эталона на плотной сетке и сторож — по трём направлениям и двум запасам (σ = 0 и σ записи сценария).
#[tokio::test]
async fn bound_covers_full_calc_on_dense_grid() {
    let dirp = std::env::temp_dir().join(format!("arb_sieve_t162_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dirp);
    std::fs::create_dir_all(&dirp).unwrap();
    let core = core(&dirp).await;
    let rec = Rec::load("sieve-t162").await;
    let mut total_cuts = 0;
    for w in [Way::CexDex, Way::DexCex, Way::CexCex] {
        for (sigma, k) in [
            (d("0"), d("0")),
            (d("0.0005"), d("1")),
            (d("0.002"), d("-1")),
        ] {
            let (n, cuts) = Box::pin(scenario(&core, &rec, w, sigma, k)).await;
            println!("{w:?} σ {sigma} k {k}: точек {n}, отсечений по порогам {cuts}");
            total_cuts += cuts;
        }
    }
    assert!(
        total_cuts > 0,
        "граница ни разу не отсекла — проверка пуста"
    );
    let _ = std::fs::remove_dir_all(&dirp);
}

/// Десятичное поля записи.
fn dv(v: &Value) -> Decimal {
    Decimal::from_str(v.as_str().unwrap()).unwrap()
}

/// 18 записанных отказов: граница по числам записи (прокси раздела Б.4 проекта) ≥ прибыли записи; отсечено 17, NXT — нет.
#[test]
fn recorded_rejects_17_of_18() {
    let all: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/cand_rejects_2026-11-16/cases.json").unwrap(),
    )
    .unwrap();
    let (mut n, mut cut, mut kept) = (0, 0, Vec::new());
    for (_, group) in all.as_object().unwrap() {
        for c in group.as_array().unwrap() {
            n += 1;
            let ticker = c["направление"]["asset"]["ticker"].as_str().unwrap();
            let dex_a = c["направление"]["buy_on"] == "bsc_dex";
            let (q, buy, sell) = (dv(&c["qty"]), dv(&c["buy_avg"]), dv(&c["sell_avg"]));
            let (inv, pro, profit) = (dv(&c["invested"]), dv(&c["proceeds"]), dv(&c["profit_usd"]));
            let qmax = dv(&c["size"]["max_qty"]);
            let lines = c["строки_затрат"].as_array().unwrap();
            let sum = |kind: &str, stage: Option<&str>, asset: Option<&str>| -> Decimal {
                lines
                    .iter()
                    .filter(|l| {
                        l["вид"] == kind
                            && stage.is_none_or(|s| l["этап"] == s)
                            && asset.is_none_or(|a| l["актив"] == a)
                    })
                    .map(|l| dv(&l["сумма"]))
                    .sum()
            };
            let c_res = sum("PriceMoveReserve", None, None) / pro;
            let deep = |p: Decimal| {
                vec![PriceLevel {
                    price: p,
                    qty: qmax * d("10"),
                }]
            };
            let (buy_m, sell_m, arrive, r_b) = if dex_a {
                let r_b = sum("TradeFee", Some("B"), Some("USDT")) / pro;
                (
                    Buy::Dex {
                        rho: Decimal::ONE / buy,
                    },
                    Sell::Book {
                        levels: deep(sell),
                        fee_quote: r_b,
                    },
                    Arrive {
                        pct: d("0"),
                        fix: d("0"),
                        keep: d("1"),
                    },
                    r_b,
                )
            } else {
                let fee_q = sum("TradeFee", Some("A"), Some("USDT")) / inv;
                let fee_c = sum("TradeFee", Some("A"), Some(ticker)) / q;
                let w = sum("WithdrawFee", None, None);
                let sent = q * (Decimal::ONE - fee_c);
                let tax = sum("TransferTax", None, None);
                let keep = if sent > w {
                    Decimal::ONE - tax / (sent - w)
                } else {
                    Decimal::ONE
                };
                (
                    Buy::Book {
                        levels: vec![PriceLevel {
                            price: buy,
                            qty: qmax,
                        }],
                        fee_quote: fee_q,
                        fee_coin: fee_c,
                    },
                    Sell::Dex { rho: sell },
                    Arrive {
                        pct: d("0"),
                        fix: w,
                        keep,
                    },
                    Decimal::ZERO,
                )
            };
            let bound = Bound {
                buy: buy_m,
                sell: sell_m,
                arrive,
                kappa: (Decimal::ONE - c_res - r_b).max(Decimal::ZERO),
                fixed: sum("UsdtReturn", None, None),
            };
            let (u, _) = bound.u_at(q).unwrap();
            assert!(
                u >= profit,
                "{ticker}: граница {u} на объёме записи < прибыли записи {profit}"
            );
            let msg = c["сообщение"].as_str().unwrap();
            let tail = msg.split("ниже порога ").nth(1).unwrap();
            let t: String = tail
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.' || *ch == '-')
                .collect();
            let pct = tail[t.len()..].starts_with('%');
            let t = Decimal::from_str(&t).unwrap();
            let best = bound.best(Some(if pct { t } else { d("-1") })).unwrap();
            let is_cut = if pct {
                best.below_pct()
            } else {
                best.below_usd(t)
            };
            println!(
                "{ticker}: прибыль {profit}, U(q) {u}, max U {} → {}",
                best.max_u,
                if is_cut {
                    "отсечён"
                } else {
                    "в расчёт"
                }
            );
            if is_cut {
                cut += 1;
            } else {
                kept.push(ticker.to_string());
            }
        }
    }
    assert_eq!(n, 18);
    assert_eq!(cut, 17, "в расчёт: {kept:?}");
    assert_eq!(kept, vec!["NXT".to_string()]);
}
