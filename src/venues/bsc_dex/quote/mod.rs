//! Файл: venues/bsc_dex/quote/mod.rs
//! Что делает: котировка на объём для контракта quote — режим dex.swap_mode: «direct» (лучший одиночный пул,
//! решение 29) или «smart» (умный маршрут PancakeSwap против прямой сделки Uniswap, решение 38 —
//! quote/smart_quote.rs). Проверяет свежесть блока («не торгуем вслепую»), добавляет газ разрешений и чаевые.
//! Цена газа = max(eth_gasPrice с надбавкой, минимум газа готовых каналов) — та же, что при отправке.
//! Кто вызывает: mod.rs (контракт quote).
//! От чего зависит: quote/direct.rs, quote/smart_quote.rs, quote/costs.rs, quote/gas.rs, check/state.rs,
//! pools/registry.rs, tokens.rs, node/heads.rs, wallet/approve.rs, send/mod.rs (минимум газа и чаевые каналов).
//! Настройки: dex.swap_mode, venue.quote_asset [bsc_dex]; разрешения — dex.universal_router, dex.permit2 [вид пула]
//! (прямой маршрут) или smart.universal_router, smart.permit2 [bsc_dex] (умный); feed.stale_after_ms [bsc_dex].

pub mod costs;
pub mod direct;
pub mod gas;
pub mod mixed_quoter;
pub mod pool_quote;
pub mod quoter;
pub mod quoter_calls;
pub mod select;
pub mod smart;
pub mod smart_quote;

use rust_decimal::Decimal;

use alloy_primitives::U256;

use crate::core::types::{Asset, Cost, CostKind, Quote, RouteMode, Side};
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
        let mode = ctx.text("dex.swap_mode")?;
        if mode != "direct" && mode != "smart" {
            return Err(ctx.err(
                ErrorCode::NotConfigured,
                format!("dex.swap_mode = «{mode}»: известны «direct» и «smart»"),
            ));
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
        // Цена газа — как при отправке: eth_gasPrice с надбавкой, не ниже минимума готовых каналов (exec/txs.rs).
        let gas = self
            .gas
            .wei(ctx, &self.heads)
            .await?
            .max(super::send::planned_floor(ctx)?);
        let usdt = ctx.text("venue.quote_asset")?;
        let taxes = direct::taxes(self, asset)?;
        let mut q = if mode == "smart" {
            let decs = (coin_dec, usdt_dec);
            smart_quote::quote(self, asset, side, qty, entries, &taxes, decs, gas, &usdt).await?
        } else {
            let (best, taxes) = direct::best(self, asset, side, qty, entries, coin_dec, usdt_dec)?;
            costs::quote(ctx, asset, side, qty, &best, &taxes, gas, &usdt)?
        };
        if let Some(units) = approve_gas(self, &q, coin_dec, usdt_dec).await? {
            let wei = gas.saturating_mul(U256::from(units));
            let bnb = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("Газ разрешений {wei} wei не переводится в BNB"),
                )
            })?;
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
/// ERC-20 → Permit2 и Permit2 → Universal Router маршрута против суммы сделки; газ — измеренный пробным обменом
/// в первом пуле маршрута (разрешения одни на токен, от пула не зависят).
/// Возвращает: единицы газа или None — разрешения уже есть. Ошибки: ключ кошелька, адреса, узел, газ не измерен.
async fn approve_gas(
    q: &Quoting,
    quote: &Quote,
    coin_dec: u8,
    usdt_dec: u8,
) -> GatewayResult<Option<u64>> {
    let ctx = &q.ctx;
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    };
    let route = quote
        .route
        .as_ref()
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "у котировки нет маршрута"))?;
    let hop = route
        .legs
        .first()
        .and_then(|l| l.hops.first())
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "в маршруте нет пула"))?;
    let e = q
        .reg
        .get(&hop.pool)
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "пул маршрута больше не в реестре"))?;
    let (token, amount) = match quote.side {
        Side::Buy => (
            e.usdt,
            num::to_units(quote.avg_price * quote.fillable_qty, usdt_dec),
        ),
        Side::Sell => (e.coin, num::to_units(quote.fillable_qty, coin_dec)),
    };
    let amount = amount.ok_or_else(bad)?;
    let (ur, permit2) = if route.mode == RouteMode::Smart {
        (
            ctx.addr("smart.universal_router")?,
            ctx.addr("smart.permit2")?,
        )
    } else {
        (
            ctx.dex_addr("dex.universal_router", e.family.id)?,
            ctx.dex_addr("dex.permit2", e.family.id)?,
        )
    };
    let wallet = signer::address(ctx)?;
    let now = q.heads.fresh(ctx)?.timestamp;
    if approve::missing(ctx, token, wallet, permit2, ur, amount, now)
        .await?
        .is_empty()
    {
        return Ok(None);
    }
    let f = q.checks.pool(&e.pool.reference);
    let g = if quote.side == Side::Buy {
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
