//! Файл: venues/bsc_dex/quote/mod.rs
//! Что делает: котировка на объём для контракта quote — режим dex.swap_mode: «direct» (лучший одиночный пул,
//! решение 29); «smart» — шаг 1e (quote/smart.rs). Проверяет свежесть блока («не торгуем вслепую»).
//! Кто вызывает: mod.rs (контракт quote), позже exec/* (перепроверка перед отправкой).
//! От чего зависит: quote/direct.rs, quote/costs.rs, quote/gas.rs, check/state.rs, pools/registry.rs, tokens.rs, node/heads.rs.
//! Настройки: dex.swap_mode, venue.quote_asset [bsc_dex], feed.stale_after_ms [bsc_dex].

pub mod costs;
pub mod direct;
pub mod gas;
pub mod pool_quote;
pub mod quoter;
pub mod select;

use rust_decimal::Decimal;

use crate::core::types::{Asset, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::check::state::Checks;
use super::ctx::Ctx;
use super::node::heads::Heads;
use super::pools::registry::Registry;
use super::tokens::Tokens;
use gas::GasPrice;

/// Всё, что нужно котировке. Дешёво клонируется.
#[derive(Clone)]
pub struct Quoting {
    pub ctx: Ctx,
    pub reg: Registry,
    pub checks: Checks,
    pub tokens: Tokens,
    pub heads: Heads,
    pub gas: GasPrice,
}

impl Quoting {
    /// Котировка. Принимает: монету, сторону, количество монет.
    /// Ошибки: NotConfigured (режим, тикер USDT), StaleData (нет свежего блока), CoinNotChecked / CoinCheckFailed,
    /// NoRoute (нет допустимого пула — причины по пулам), узел (decimals, цена газа).
    pub async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        let ctx = &self.ctx;
        match ctx.text("dex.swap_mode")?.as_str() {
            "direct" => {}
            m => {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    format!("Режим обмена «{m}» — шаг 1e (умный маршрут), пока только «direct»"),
                ));
            }
        }
        self.heads.fresh(ctx)?;
        let entries = self.reg.by_ticker(&asset.ticker);
        let first = entries.first().ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                format!("{}: пулов с USDT нет", asset.ticker),
            )
        })?;
        let coin_dec = self.tokens.decimals(ctx, first.coin).await?;
        let usdt_dec = self.tokens.decimals(ctx, first.usdt).await?;
        let (best, taxes) = direct::best(self, asset, side, qty, entries, coin_dec, usdt_dec)?;
        let gas = self.gas.wei(ctx, &self.heads).await?;
        costs::quote(
            ctx,
            asset,
            side,
            qty,
            &best,
            &taxes,
            gas,
            &ctx.text("venue.quote_asset")?,
        )
    }
}
