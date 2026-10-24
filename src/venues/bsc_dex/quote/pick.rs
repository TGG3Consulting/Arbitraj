//! Файл: venues/bsc_dex/quote/pick.rs
//! Что делает: выбор лучшей из двух котировок одной стороны — сначала больше исполнимое количество, затем лучше итог
//! в USDT после газа (продажа — больше получим, покупка — меньше отдадим). Газ в USDT — продажей суммы газа в BNB
//! в пул «BNB — USDT» реестра (монета или промежуточная с тикером BNB); нет такого пула — сравнение без газа.
//! Кто вызывает: quote/mod.rs (прямой пул против пути через промежуточную), quote/smart_quote.rs (умный против Uniswap).
//! От чего зависит: quote/pool_quote.rs, pools/registry.rs, tokens.rs, num.rs.
//! Настройки: нет своих.
//!
//! Газ BSC на сделку — доли цента; сравнение без газа, когда пула BNB нет, — вопрос Тиграну в Issue #7 (шаг 1e).

use rust_decimal::Decimal;

use crate::core::types::{Quote, Side};

use super::super::num;
use super::super::wallet::balances::BNB;
use super::{Quoting, pool_quote};

/// Больший итог из двух котировок. Ошибок нет.
pub async fn pick(q: &Quoting, a: Quote, b: Quote, side: Side, usdt_dec: u8) -> Quote {
    if a.fillable_qty != b.fillable_qty {
        return if a.fillable_qty > b.fillable_qty {
            a
        } else {
            b
        };
    }
    let (ga, gb) = match (
        gas_usdt(q, &a, usdt_dec).await,
        gas_usdt(q, &b, usdt_dec).await,
    ) {
        (Some(x), Some(y)) => (x, y),
        _ => (Decimal::ZERO, Decimal::ZERO),
    };
    let (va, vb) = (a.avg_price * a.fillable_qty, b.avg_price * b.fillable_qty);
    let a_better = match side {
        Side::Sell => va - ga >= vb - gb,
        Side::Buy => va + ga <= vb + gb,
    };
    if a_better { a } else { b }
}

/// Затраты котировки в BNB (не в цене), переведённые в USDT по пулу «BNB — USDT» реестра. None — нет пула.
async fn gas_usdt(q: &Quoting, x: &Quote, usdt_dec: u8) -> Option<Decimal> {
    let ctx = &q.ctx;
    let usdt = q.tokens.usdt(ctx).ok()?;
    let bnb: Decimal = x
        .costs
        .iter()
        .filter(|c| c.asset == BNB && !c.in_price)
        .map(|c| c.amount)
        .sum();
    for e in q
        .reg
        .by_ticker(BNB)
        .into_iter()
        .filter(|e| e.valid && e.usdt == usdt)
    {
        let Ok(dec) = q.tokens.decimals(ctx, e.coin).await else {
            continue;
        };
        let Some(units) = num::to_units(bnb, dec) else {
            continue;
        };
        if let Ok(s) = pool_quote::swap(ctx, &e, e.coin, true, units) {
            return num::to_decimal(s.amount_out, usdt_dec);
        }
    }
    None
}
