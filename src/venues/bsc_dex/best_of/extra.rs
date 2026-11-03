//! Файл: venues/bsc_dex/best_of/extra.rs
//! Что делает: затраты котировки вне средней цены в USDT (решение 123) — для честного сравнения «лучшего из двух»:
//! газ в BNB (газ обмена, approve, обёртка BNB, чаевые канала — у своей; gas × gasPrice ответа /routes — у KyberSwap)
//! переводится продажей суммы BNB в пул «BNB — USDT» реестра (quote/pick.rs gas_usdt), затраты в USDT (газ KyberSwap
//! из gasUsd) — как есть, в монете — по средней цене котировки. Затраты «в цене» (комиссия пула, налог) уже внутри
//! средней и не добавляются.
//! Кто вызывает: best_of/quote.rs, best_of/trade.rs.
//! От чего зависит: quote/pick.rs, tokens.rs, wallet/balances.rs (BNB), core/types.
//! Настройки: venue.quote_asset [bsc_dex] (тикер USDT).

use rust_decimal::Decimal;

use crate::core::types::Quote;

use super::super::quote::{Quoting, pick};
use super::super::wallet::balances::BNB;

/// Затраты вне цены в USDT. Принимает: окружение котировки, котировку. Возвращает: сумму; None — BNB не переведён
/// (нет пула «BNB — USDT»), тикер USDT не задан или затрата в неизвестной валюте. Ошибок нет.
pub async fn usdt(q: &Quoting, x: &Quote) -> Option<Decimal> {
    let ctx = &q.ctx;
    let usdt_ticker = ctx.text("venue.quote_asset").ok()?;
    let mut sum = Decimal::ZERO;
    let mut bnb = false;
    for c in x
        .costs
        .iter()
        .filter(|c| !c.in_price && !c.amount.is_zero())
    {
        if c.asset == BNB {
            bnb = true;
        } else if c.asset == usdt_ticker {
            sum += c.amount;
        } else if c.asset == x.asset.ticker {
            sum += c.amount * x.avg_price;
        } else {
            return None;
        }
    }
    if bnb {
        let token = q.tokens.usdt(ctx).ok()?;
        let dec = q.tokens.decimals(ctx, token).await.ok()?;
        sum += pick::gas_usdt(q, x, dec).await?;
    }
    Some(sum)
}
