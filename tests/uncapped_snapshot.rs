//! Файл: tests/uncapped_snapshot.rs
//! Что проверяет: решение 217 — расчёт «без потолка» по сделке считается по снимку рынка на момент решения о входе,
//! а не по живым данным после нашей покупки. Данные — записанные (tests/size_common): стакан BingX CAKE-USDT@depth20
//! (ws_spot_2026-10-23) и пул CAKE/USDT PancakeSwap V2 (getReserves на блоке 0x7b936ae).
//! - Биржа: снимок стакана (gateway/frozen.rs FrozenBook — тот же код, что живая котировка бирж) взят до покупки;
//!   потом наша покупка съедает лучший уровень живого стакана (как сделка MCRT 07.11: весь уровень) — котировка и
//!   худшая цена по снимку не меняются, по живому стакану — хуже.
//! - DEX: снимок записей реестра пулов (pools/registry_frozen.rs) взят до свопа; потом живой реестр получает состояние
//!   после нашего свопа — обмен по замороженному реестру не меняется, по живому — хуже.
//! - Расчёт «без потолка» (core/size_uncapped.rs) на данных снимков после изменения живого рынка совпадает с расчётом
//!   на рынке до покупки, а на живых данных после покупки — занижен (как записи до решения 217).
//!
//! Ставки taker аккаунта в записях нет — у снимка стакана ставка 0 (комиссия проверяется отдельно на плане комиссии).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::frozen, arb::venues::bsc_dex::pools::{registry, registry_frozen},
//! arb::venues::bsc_dex::quote::pool_quote, arb::core::size_uncapped, tests/size_common.

mod size_common;

use alloy_primitives::U256;
use arb::core::size_uncapped::uncapped;
use arb::core::types::{Asset, OrderBook, PoolRef, PoolState, Side, VenueId};
use arb::core::uncapped_types::{BASIS_LIVE_AFTER_BUY, BASIS_SNAPSHOT, understated};
use arb::gateway::frozen::{BookFee, FrozenBook, FrozenQuote, deepest, walk};
use arb::venues::bsc_dex::num::to_units;
use arb::venues::bsc_dex::pools::registry::Registry;
use arb::venues::bsc_dex::pools::registry_frozen::Want;
use arb::venues::bsc_dex::quote::pool_quote::swap;
use chrono::Utc;
use rust_decimal::Decimal;
use size_common::curve::{Dir, RecCurve};
use size_common::{CAKE, PAIR, Rec, USDT, d, params};

/// Стакан ядра из записанных уровней CAKE.
fn book(rec: &Rec) -> OrderBook {
    OrderBook {
        venue: VenueId("bingx".into()),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        bids: rec.bids.clone(),
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: Utc::now(),
    }
}

/// Снимок стакана покупки (ставка тейкера — по плану комиссии; в записях её нет).
fn frozen(b: OrderBook, rate: Decimal) -> FrozenBook {
    FrozenBook {
        venue: b.venue.clone(),
        asset: b.asset.clone(),
        side: Side::Buy,
        book: b,
        fee: BookFee {
            rate,
            per_coin: false,
            asset: "USDT".into(),
            paid_in: None,
        },
        taken_at: Utc::now(),
    }
}

#[tokio::test]
async fn cex_snapshot_ignores_eaten_level() {
    let rec = Rec::load("snap-cex").await;
    let mut live = book(&rec);
    let snap = frozen(live.clone(), Decimal::ZERO);
    // Наша покупка — весь лучший уровень (как MCRT 07.11: план и факт — весь уровень).
    let qty = live.asks[0].qty;
    let before = snap.quote(qty).await.unwrap();
    let (filled, notional) = walk(&rec.asks, qty);
    assert_eq!(
        (before.fillable_qty, before.avg_price),
        (filled, notional / filled)
    );
    assert_eq!(
        before.avg_price, rec.asks[0].price,
        "весь объём — на лучшем уровне"
    );
    assert_eq!(deepest(&snap.book, Side::Buy, qty), Some(rec.asks[0].price));
    // Живой стакан после нашей покупки: лучший уровень съеден.
    live.asks.remove(0);
    let after = snap.quote(qty).await.unwrap();
    assert_eq!(
        (after.avg_price, after.fillable_qty),
        (before.avg_price, before.fillable_qty)
    );
    let (lf, ln) = walk(&live.asks, qty);
    assert!(
        ln / lf > before.avg_price,
        "по живому стакану после покупки — дороже"
    );
    assert!(deepest(&live, Side::Buy, qty) > deepest(&snap.book, Side::Buy, qty));
    // План комиссии: от суммы сделки в котируемой монете, от количества — в монете.
    let fee = frozen(book(&rec), d("0.001")).price(qty);
    assert_eq!(fee.costs[0].amount, notional * d("0.001"));
    assert_eq!(fee.costs[0].asset, "USDT");
    assert!(snap.describe().contains("bingx CAKE"));
}

#[tokio::test]
async fn dex_snapshot_ignores_our_swap() {
    let rec = Rec::load("snap-dex").await;
    let reg = Registry::default();
    reg.put(rec.pool.clone());
    let snap = reg.freeze(&Want {
        coin: CAKE,
        mids: &[],
        usdt: USDT,
        wbnb: None,
        paths: &[],
    });
    assert_eq!(snap.entries.len(), 1);
    let fz = Registry::frozen_from(&snap, None);
    let r = PoolRef::Contract(PAIR);
    let amount = to_units(d("100"), 18).unwrap();
    let buy = |reg: &Registry| swap(&rec.ctx, &reg.get(&r).unwrap(), USDT, true, amount).unwrap();
    let before = buy(&fz).amount_out;
    // Наш своп «USDT → CAKE» на 5000 USDT сдвигает пул живого реестра (резервы V2 после обмена).
    let ours = buy_out(&rec, &reg, &r, to_units(d("5000"), 18).unwrap());
    reg.update(&r, |e| {
        if let PoolState::V2 { reserve0, reserve1 } = &mut e.pool.state {
            *reserve0 -= ours;
            *reserve1 += to_units(d("5000"), 18).unwrap();
        }
    });
    assert_eq!(
        buy(&fz).amount_out,
        before,
        "замороженный реестр не видит наш своп"
    );
    assert!(
        buy(&reg).amount_out < before,
        "живой реестр после свопа — монет меньше"
    );
    // Запись реестра в снимке не изменилась (копия при записи).
    assert_eq!(fz.get(&r).unwrap().pool.state, rec.pool.pool.state);
}

/// Выход CAKE свопа USDT → CAKE на живом реестре. Ошибок нет (паника — расчёт невозможен).
fn buy_out(rec: &Rec, reg: &Registry, r: &PoolRef, usdt: U256) -> U256 {
    swap(&rec.ctx, &reg.get(r).unwrap(), USDT, true, usdt)
        .unwrap()
        .amount_out
}

#[tokio::test]
async fn uncapped_on_snapshot_not_affected_by_buy() {
    let ladder = [d("20"), d("100"), d("1000")];
    let budget = d("20");
    // Рынок до покупки — эталон.
    let rec = Rec::load("snap-base").await;
    let dir = Dir::BingxToDex;
    let mut c = RecCurve {
        rec: &rec,
        dir,
        slip: Decimal::ONE,
    };
    let base = uncapped(
        &mut c,
        &params(&rec, dir, d("1000000")),
        budget,
        &ladder,
        |_| true,
    )
    .await
    .unwrap();
    // Снимок берётся до покупки; затем живой стакан теряет лучший уровень (наша покупка).
    let snap = frozen(book(&rec), Decimal::ZERO);
    let mut live = Rec::load("snap-live").await;
    live.asks.remove(0);
    // По снимку (уровни стакана снимка) — как до покупки.
    let mut on_snap = Rec::load("snap-frozen").await;
    on_snap.asks = snap.book.asks.clone();
    let mut c = RecCurve {
        rec: &on_snap,
        dir,
        slip: Decimal::ONE,
    };
    let s = uncapped(
        &mut c,
        &params(&on_snap, dir, d("1000000")),
        budget,
        &ladder,
        |_| true,
    )
    .await
    .unwrap();
    assert_eq!((s.opt.qty, s.opt.profit), (base.opt.qty, base.opt.profit));
    // На живых данных после покупки — занижено (так считали записи до решения 217).
    let mut c = RecCurve {
        rec: &live,
        dir,
        slip: Decimal::ONE,
    };
    let l = uncapped(
        &mut c,
        &params(&live, dir, d("1000000")),
        budget,
        &ladder,
        |_| true,
    )
    .await
    .unwrap();
    assert!(
        l.opt.profit < base.opt.profit,
        "после покупки: {} против {}",
        l.opt.profit,
        base.opt.profit
    );
    // Признак старых записей сделок.
    assert!(understated("deal", BASIS_LIVE_AFTER_BUY));
    assert!(!understated("deal", BASIS_SNAPSHOT));
    assert!(!understated("accept", BASIS_LIVE_AFTER_BUY));
}
