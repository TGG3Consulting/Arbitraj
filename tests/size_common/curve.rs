//! Файл: tests/size_common/curve.rs
//! Что делает: кривая «прибыль от объёма» CAKE на записанных данных из функций ядра и адаптеров (решение 66):
//! стакан — bingx::book::walk; пул — адаптер DEX quote/pool_quote::swap (NoRoute → core::costs_quote::classify);
//! пределы ордера по худшей цене — core::costs_quote::order_limits; шаг — floor_step; точность вывода —
//! core::costs_withdraw::floor_precision. Списание по бюджету — количество × худшая цена (как Calc.spend_max).
//! Кто вызывает: tests/core_size_optimum.rs, tests/core_size_limits.rs.
//! От чего зависит: tests/size_common/mod.rs.

use arb::core::costs_quote::{classify, floor_step, order_limits};
use arb::core::costs_withdraw::floor_precision;
use arb::core::size::{Curve, Fail, Params, Point};
use arb::core::size_curve::cumulative;
use arb::core::types::PriceLevel;
use arb::venues::bingx::book::walk;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use async_trait::async_trait;
use rust_decimal::Decimal;

use super::{CAKE, Rec, USDT, d};

/// Направление сделки.
#[derive(Clone, Copy, Debug)]
pub enum Dir {
    /// Купить в пуле, перевести на BingX, продать по bids (как живой случай WFI bsc_dex → bingx).
    DexToBingx,
    /// Купить по asks BingX, вывести с BingX, продать в пул.
    BingxToDex,
}

/// Кривая по записям; slip — exec.dex_slippage_pct теста (худшая цена покупки на DEX).
pub struct RecCurve<'a> {
    pub rec: &'a Rec,
    pub dir: Dir,
    pub slip: Decimal,
}

/// Цена самого глубокого уровня, нужного на qty (лимитная цена ордера, как market_read::deepest_price).
pub fn deepest(levels: &[PriceLevel], qty: Decimal) -> Option<Decimal> {
    let mut left = qty;
    levels
        .iter()
        .find(|l| {
            left -= l.qty;
            left <= Decimal::ZERO
        })
        .map(|l| l.price)
}

/// Проход по стакану с проверкой глубины: (сумма, худшая цена). Ошибки: TooLarge.
fn book(levels: &[PriceLevel], qty: Decimal) -> Result<(Decimal, Decimal), Fail> {
    let (filled, sum) = walk(levels, qty);
    match deepest(levels, qty) {
        Some(w) if filled >= qty => Ok((sum, w)),
        _ => Err(Fail::TooLarge(format!(
            "bingx: глубины хватает на {filled} из {qty}"
        ))),
    }
}

#[async_trait]
impl Curve for RecCurve<'_> {
    type Data = ();

    async fn at(&mut self, q: Decimal) -> Result<Point<()>, Fail> {
        let r = self.rec;
        let p = Some(&r.pair);
        let units = |x: Decimal| to_units(x, 18).unwrap();
        let dec = |x| to_decimal(x, 18).unwrap();
        let (spend, cost, arrive, proceeds) = match self.dir {
            Dir::DexToBingx => {
                let s = swap(&r.ctx, &r.pool, USDT, false, units(q))
                    .map_err(|e| classify("bsc_dex", &e))?;
                let usdt = dec(s.amount_in);
                let spend = usdt * (Decimal::ONE + self.slip / Decimal::ONE_HUNDRED);
                if r.net.deposit_min.is_some_and(|m| q < m) {
                    return Err(Fail::TooSmall(format!(
                        "приход {q} меньше минимального депозита"
                    )));
                }
                let sell = floor_step(q, p);
                if sell.is_zero() {
                    return Err(Fail::TooSmall("приход меньше шага".into()));
                }
                let (sum, worst) = book(&r.bids, sell)?;
                order_limits("bingx", p, sell, worst)?;
                (spend, usdt, q, sum)
            }
            Dir::BingxToDex => {
                let (sum, worst) = book(&r.asks, q)?;
                order_limits("bingx", p, q, worst)?;
                let send = floor_precision(q, Some(&r.net));
                if r.net.withdraw_min.is_some_and(|m| send < m) {
                    return Err(Fail::TooSmall(format!("вывод {send} меньше минимума")));
                }
                if r.net.withdraw_max.is_some_and(|m| send > m) {
                    return Err(Fail::TooLarge(format!("вывод {send} больше максимума")));
                }
                let arrive = send - r.net.withdraw_fee.unwrap();
                let s = swap(&r.ctx, &r.pool, CAKE, true, units(arrive))
                    .map_err(|e| classify("bsc_dex", &e))?;
                (q * worst, sum, arrive, dec(s.amount_out))
            }
        };
        Ok(Point {
            qty: q,
            invested: spend,
            arrive,
            profit: proceeds - cost,
            data: (),
        })
    }
}

/// Параметры с предлагаемыми значениями size.*.
pub fn params(rec: &Rec, dir: Dir, budget: Decimal) -> Params {
    // Лучшая цена: у биржи — лучший ask; у DEX — цена пула по резервам без комиссии (как оценка ядра).
    let (price, buy, sell) = match dir {
        Dir::DexToBingx => (pool_price(rec), vec![], cumulative(&rec.bids)),
        Dir::BingxToDex => (rec.asks[0].price, cumulative(&rec.asks), vec![]),
    };
    Params {
        step: rec.pair.step_size,
        budget,
        guess: budget / price,
        buy_breaks: buy,
        sell_breaks: sell,
        bisect_steps: 32,
        grid_points: 16,
        refine_steps: 30,
        tolerance_pct: d("0.01"),
        max_evals: 300,
    }
}

/// Цена пула USDT за CAKE по резервам.
fn pool_price(rec: &Rec) -> Decimal {
    use arb::core::types::PoolState;

    let PoolState::V2 { reserve0, reserve1 } = rec.pool.pool.state else {
        unreachable!()
    };
    to_decimal(reserve1, 18).unwrap() / to_decimal(reserve0, 18).unwrap()
}
