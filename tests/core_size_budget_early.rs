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
//! Решение 380 (t166): sell_max_caps_size_search — предел объёма продажи DirCtx.sell_max на тех же записях;
//! sell_max_below_min_rejects — предел ниже минимума ордера BingX: отказ подбора, без паники; ревью Н2 — refit::limit.
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
        chain: None,
        ticker: "CAKE".into(),
        contract: Some(CAKE),
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
        units_b_per_a: Default::default(),
        dir: dir(),
        a: p("bingx", Some(rec.net.clone())),
        b: p("bsc_dex", None),
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            chain: None,
            ticker: "USDT".into(),
            contract: Some(USDT),
        },
        cex: vec![],
        sigma: d("0.0005"),
        transit_s: d("20"),
        k: d("1"),
        h: d("0.7"),
        closure_share: None,
        keep: arb::core::reserve::keep_of(d("1"), d("0.0005"), d("20")),
        sigma_check: None,
        return_per_trade: false,
        pair_a: Some(rec.pair.clone()),
        pair_b: None,
        usdt_b: None,
        transfer_tax: Some(Decimal::ZERO),
        sell_max: None,
        band_a: None,
        band_b: None,
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

/// Регрессия частичной покупки: на тех же записанных стакане и пуле вывод проходит ровно на
/// чистом остатке. Повторный расчёт покупки с этого остатка теряет комиссию ещё раз и нарушает минимум.
#[tokio::test]
async fn held_coin_keeps_the_withdrawal_threshold_without_a_buy_book() {
    let dirp = std::env::temp_dir().join(format!("arb_held_qty_{}", std::process::id()));
    std::fs::create_dir_all(&dirp).unwrap();
    let core = core(&dirp).await;
    let rec = Rec::load("held-qty").await;
    let mut c = cx(&rec);
    let mut book = frozen_buy(&rec);
    let snapshot = |book: FrozenBook| Frozen {
        a: Arc::new(book.clone()),
        b: Arc::new(book),
        at: Utc::now(),
        decided: None,
    };
    let full = snapshot(book.clone());
    let src = Src {
        live: Live {
            core: &core,
            src: Some(&full),
        },
        rec: &rec,
        dex: AtomicU32::new(0),
    };
    let plan = costs::evaluate_with(&c, d("10"), &src).await.unwrap();
    let net = d("9.99");
    assert_eq!(plan.buy_quote.costs[0].amount, plan.qty - net);
    c.a.info.as_mut().unwrap().withdraw_min = Some(net);
    let duplicate_fee = costs::evaluate_with(&c, net, &src).await.unwrap_err();
    assert!(matches!(duplicate_fee, Fail::TooSmall(t) if t.contains("вывод")));

    // Сторона A больше не предлагает покупку. Та же будущая продажа должна дать ту же выручку
    // после вывода, налога и запаса. При прежнем расчёте отказ был бы уже на котировке A.
    book.book.asks.clear();
    let empty = snapshot(book);
    let after = Src {
        live: Live {
            core: &core,
            src: Some(&empty),
        },
        rec: &rec,
        dex: AtomicU32::new(0),
    };
    c.pair_a = None;
    assert!(costs::evaluate_with(&c, net, &after).await.is_err());
    let value = costs::held_value(&c, net, plan.buy_avg, &after)
        .await
        .unwrap();
    assert_eq!(value, plan.proceeds - plan.costs_usdt);
    assert_eq!(after.dex.load(Ordering::Relaxed), 2);
    c.a.info.as_mut().unwrap().withdraw_min = Some(plan.qty);
    let still_too_small = costs::held_value(&c, net, plan.buy_avg, &after)
        .await
        .unwrap_err();
    assert!(matches!(still_too_small, Fail::TooSmall(t) if t.contains("вывод")));

    // Комиссия в USDT либо уплаченная монетой скидки оставляет все 10 CAKE для вывода.
    // Сравниваем с тем же планом на тех же данных, исключив только строку уже исполненной покупки.
    for paid_in in [None, Some("GT".to_string())] {
        let mut quoted = frozen_buy(&rec);
        quoted.fee.per_coin = false;
        quoted.fee.asset = "USDT".to_string();
        quoted.fee.paid_in = paid_in;
        let paid = snapshot(quoted);
        let before = Src {
            live: Live {
                core: &core,
                src: Some(&paid),
            },
            rec: &rec,
            dex: AtomicU32::new(0),
        };
        let with_fee = costs::evaluate_with(&c, plan.qty, &before).await.unwrap();
        let charged = with_fee
            .costs
            .iter()
            .find(|line| line.stage == "A")
            .unwrap()
            .usdt
            .unwrap();
        assert!(charged > Decimal::ZERO);
        let future = costs::held_value(&c, plan.qty, with_fee.buy_avg, &after)
            .await
            .unwrap();
        assert_eq!(future, with_fee.proceeds - (with_fee.costs_usdt - charged));
    }
}

/// Нормализованные числа из исполнения BingX PROM 23.11.2026. Заявка, отправка и приход
/// сверены отдельно. Идентификаторы счёта, ордера и транзакции для воспроизведения не нужны.
#[tokio::test]
async fn recorded_prom_buy_leaves_the_quantity_that_was_actually_withdrawn() {
    use arb::core::{deal::Deal, fills, types::TradeResult};
    use serde_json::json;

    let dirp = std::env::temp_dir().join(format!("arb_held_prom_{}", std::process::id()));
    std::fs::create_dir_all(&dirp).unwrap();
    let core = core(&dirp).await;
    let fact: TradeResult = serde_json::from_value(json!({
        "request": {"venue": "bingx", "asset": {"ticker": "PROM"}, "side": "Buy",
                    "qty": "52.601", "worst_price": "6.107", "deal_id": "", "route": null},
        "status": "PartiallyFilled", "filled_qty": "2.065", "avg_price": "6.107",
        "fees": [{"asset": "PROM", "amount": "0.002065"}], "venue_ref": "", "chain": null,
        "finished_at": "2026-11-23T00:00:00Z"
    }))
    .unwrap();
    let direction = Direction {
        asset: fact.request.asset.clone(),
        buy_on: fact.request.venue.clone(),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut deal = Deal::new(String::new(), direction, json!({}), Decimal::ZERO);
    fills::trade(&core, &mut deal, &fact).await;
    assert_eq!(deal.invested, d("12.610955"));
    assert_eq!(deal.spent, Decimal::ZERO);
    assert_eq!(deal.qty, d("2.06293500"));
    assert_eq!(deal.qty - d("0.003600"), d("2.059335"));
    let twice = (deal.qty * d("0.999")).trunc_with_scale(8);
    assert_eq!(twice, d("2.06087206"));
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

/// Расчёт одного объёма с бюджетом у кривой (как C::at). Ошибки: Fail расчёта.
async fn at(cx: &DirCtx, qty: Decimal, src: &Src<'_>, budget: Decimal) -> Result<Calc, Fail> {
    costs::evaluate_capped(cx, qty, src, Some(budget)).await
}

/// Решение 380 (задача t166): предел «объём продажи ≤ проверяемого симуляцией» (DirCtx.sell_max →
/// core/precheck_dex_cut.rs sell_max_ok в costs.rs). На тех же записанных данных (стакан BingX CAKE, пул CAKE/USDT
/// PancakeSwap V2): объём сверх предела — Fail::TooLarge с причиной; подбор с пределом выбирает объём, у которого
/// продажа не больше предела и объём меньше выбранного без предела; без предела выбор прежний. Предел — половина
/// продажи выбора без предела (записанного объёма живого случая NXT 17.11 с этими пулами нет — монета другая).
#[tokio::test]
async fn sell_max_caps_size_search() {
    let dirp = std::env::temp_dir().join(format!("arb_sell_max_380_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dirp);
    std::fs::create_dir_all(&dirp).unwrap();
    let core = core(&dirp).await;
    let rec = Rec::load("sell-max-380").await;
    let fz = Frozen {
        a: Arc::new(frozen_buy(&rec)),
        b: Arc::new(frozen_buy(&rec)),
        at: Utc::now(),
        decided: None,
    };
    let src = Src {
        live: Live {
            core: &core,
            src: Some(&fz),
        },
        rec: &rec,
        dex: AtomicU32::new(0),
    };
    let budget = d("1000");
    let p = size_common::params(&rec, Dir::BingxToDex, budget);
    let free = cx(&rec);
    let mut curve = C {
        cx: &free,
        src: &src,
        budget: Some(budget),
    };
    let old = search(&mut curve, &p).await.unwrap();
    // Предел — из самой записи: посередине между продажей на наименьшем допустимом объёме подбора (минимумы биржи) и
    // продажей на наибольшем (choice.max_qty). Продажа — в монетах ПРИХОДА (покупка − комиссии в монете), её нельзя
    // сравнивать с minQty покупки. CI e0462130: выбор без предела на этой записи — продажа 2.469328, а minQty BingX
    // 2.672 (покупка): прежнее предусловие «продажа выбора > minQty» смешивало единицы.
    let s_min = at(&free, old.choice.min_qty, &src, budget)
        .await
        .unwrap()
        .sell_qty;
    let s_max = at(&free, old.choice.max_qty, &src, budget)
        .await
        .unwrap()
        .sell_qty;
    assert!(
        s_max > s_min,
        "запись не годится: продажа на наибольшем объёме {s_max} не больше, чем на наименьшем {s_min}"
    );
    let limit = ((s_min + s_max) / d("2")).round_dp(6);
    let mut capped = cx(&rec);
    capped.sell_max = Some(limit);
    // Наибольший объём без предела — сверх предела: «слишком большой» с причиной решения 380.
    match at(&capped, old.choice.max_qty, &src, budget).await {
        Err(Fail::TooLarge(t)) => assert!(t.contains("решение 380"), "{t}"),
        other => panic!(
            "ожидался TooLarge, получено {:?}",
            other.map(|c| c.sell_qty)
        ),
    }
    let mut curve = C {
        cx: &capped,
        src: &src,
        budget: Some(budget),
    };
    let new = search(&mut curve, &p).await.unwrap();
    println!(
        "без предела: объём {} (границы {}..{}) продажа {}; предел {limit} (продажа {s_min}..{s_max}): объём {} \
         (границы {}..{}) продажа {}",
        old.choice.qty,
        old.choice.min_qty,
        old.choice.max_qty,
        old.best.data.sell_qty,
        new.choice.qty,
        new.choice.min_qty,
        new.choice.max_qty,
        new.best.data.sell_qty
    );
    // Предел режет верхнюю границу подбора, выбор — не больше предела и не меньше минимума.
    assert!(new.best.data.sell_qty <= limit);
    assert!(new.choice.max_qty < old.choice.max_qty);
    assert!(new.choice.qty >= old.choice.min_qty);
    // Прибыль — расчёта на выбранном объёме (не перенесена с прежнего).
    assert_eq!(new.best.qty, new.choice.qty);
    assert_eq!(new.best.profit, new.best.data.profit_usd);
    // Без предела — тот же выбор, что и до решения 380 (поле None ничего не меняет).
    let mut curve = C {
        cx: &free,
        src: &src,
        budget: Some(budget),
    };
    let again = search(&mut curve, &p).await.unwrap();
    assert_eq!(again.choice.qty, old.choice.qty);
    // Ревью t166 Н2: подбор под свободный остаток (core/refit.rs) с пределом refit::limit = продажа проверенного плана:
    // объём, который без предела был допустим (наибольший), с ним — «слишком большой», и выбор не продаёт больше
    // проверенного (как при упавшей цене: тот же предел списания купил бы больше монет).
    let checked = new.best.data.clone();
    let mut refit_cx = cx(&rec);
    arb::core::refit::limit(&mut refit_cx, &checked);
    assert_eq!(refit_cx.sell_max, Some(checked.sell_qty));
    assert!(matches!(
        at(&refit_cx, old.choice.max_qty, &src, budget).await,
        Err(Fail::TooLarge(_))
    ));
    let mut curve = C {
        cx: &refit_cx,
        src: &src,
        budget: Some(budget),
    };
    let refit = search(&mut curve, &p).await.unwrap();
    assert!(refit.best.data.sell_qty <= checked.sell_qty);
    // Предел уже задан (урезание 380) — берётся меньший.
    let mut both = cx(&rec);
    both.sell_max = Some(checked.sell_qty / d("2"));
    arb::core::refit::limit(&mut both, &checked);
    assert_eq!(both.sell_max, Some(checked.sell_qty / d("2")));
    let _ = std::fs::remove_dir_all(&dirp);
}

/// Решение 380, случай из CI f0587cfa: предел продажи ниже минимума ордера биржи (наименьший допустимый объём уже
/// продаёт больше предела) — подбор отказывает понятной причиной (Fail, без паники и без зацикливания; ядро
/// превращает её в отказ «пересчёт на проверяемый объём …», core/precheck_dex_cut.rs).
#[tokio::test]
async fn sell_max_below_min_rejects() {
    let dirp = std::env::temp_dir().join(format!("arb_sell_max_min_380_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dirp);
    std::fs::create_dir_all(&dirp).unwrap();
    let core = core(&dirp).await;
    let rec = Rec::load("sell-max-min-380").await;
    let fz = Frozen {
        a: Arc::new(frozen_buy(&rec)),
        b: Arc::new(frozen_buy(&rec)),
        at: Utc::now(),
        decided: None,
    };
    let src = Src {
        live: Live {
            core: &core,
            src: Some(&fz),
        },
        rec: &rec,
        dex: AtomicU32::new(0),
    };
    let budget = d("1000");
    let p = size_common::params(&rec, Dir::BingxToDex, budget);
    let free = cx(&rec);
    let mut curve = C {
        cx: &free,
        src: &src,
        budget: Some(budget),
    };
    let old = search(&mut curve, &p).await.unwrap();
    let mut capped = cx(&rec);
    capped.sell_max = Some((old.choice.min_qty / d("2")).round_dp(6));
    let mut curve = C {
        cx: &capped,
        src: &src,
        budget: Some(budget),
    };
    match search(&mut curve, &p).await {
        Err(Fail::TooSmall(t)) | Err(Fail::TooLarge(t)) => {
            println!("предел ниже минимума: {t}");
            assert!(!t.is_empty());
        }
        Err(e) => panic!("неожиданный вид отказа: {e}"),
        Ok(f) => panic!(
            "ожидался отказ, выбран объём {} (продажа {})",
            f.choice.qty, f.best.data.sell_qty
        ),
    }
    let _ = std::fs::remove_dir_all(&dirp);
}
