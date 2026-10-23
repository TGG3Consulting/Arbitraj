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
pub mod quoter_calls;
pub mod select;

use rust_decimal::Decimal;

use alloy_primitives::U256;

use crate::core::types::{Asset, Cost, CostKind, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::check::state::Checks;
use super::ctx::Ctx;
use super::node::heads::Heads;
use super::num;
use super::pools::registry::Registry;
use super::tokens::Tokens;
use super::wallet::balances::BNB;
use super::wallet::{approve, signer};
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
        let approve = approve_gas(self, &best, side, qty, coin_dec, usdt_dec).await?;
        let usdt = ctx.text("venue.quote_asset")?;
        let mut q = costs::quote(ctx, asset, side, qty, &best, &taxes, gas, &usdt)?;
        if let Some(units) = approve {
            let bnb = num::to_decimal(gas.saturating_mul(U256::from(units)), num::BNB_DECIMALS)
                .unwrap_or_default();
            q.costs
                .push(Cost::new(CostKind::ApproveGas, bnb, BNB, false));
        }
        if let Some(tip) =
            super::send::planned_tip(ctx).and_then(|w| num::to_decimal(w, num::BNB_DECIMALS))
        {
            q.costs
                .push(Cost::new(CostKind::ChannelTip, tip, BNB, false));
        }
        Ok(q)
    }
}

/// Газ разрешений, если их у кошелька ещё нет (решение 35: «газ approve, первый раз на монету»): текущие разрешения
/// ERC-20 → Permit2 и Permit2 → Universal Router против суммы сделки; газ — измеренный пробным обменом.
/// Возвращает: единицы газа или None — разрешения уже есть. Ошибки: ключ кошелька, адреса, узел, газ не измерен.
async fn approve_gas(
    q: &Quoting,
    c: &direct::Candidate,
    side: Side,
    qty: Decimal,
    coin_dec: u8,
    usdt_dec: u8,
) -> GatewayResult<Option<u64>> {
    let ctx = &q.ctx;
    let e = &c.entry;
    let (token, amount) = match side {
        Side::Buy => (e.usdt, num::to_units(c.usdt, usdt_dec)),
        Side::Sell => (e.coin, num::to_units(qty, coin_dec)),
    };
    let amount = amount.ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    })?;
    let wallet = signer::address(ctx)?;
    let ur = ctx.dex_addr("dex.universal_router", e.family.id)?;
    let permit2 = ctx.dex_addr("dex.permit2", e.family.id)?;
    let now = q.heads.fresh(ctx)?.timestamp;
    if approve::missing(ctx, token, wallet, permit2, ur, amount, now)
        .await?
        .is_empty()
    {
        return Ok(None);
    }
    let f = q.checks.pool(&e.pool.reference);
    let g = if side == Side::Buy {
        f.gas_approve_buy
    } else {
        f.gas_approve_sell
    };
    g.map(Some).ok_or_else(|| {
        ctx.err(
            ErrorCode::CoinNotChecked,
            "газ разрешений не измерен пробным обменом",
        )
    })
}
