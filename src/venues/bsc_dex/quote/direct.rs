//! Файл: venues/bsc_dex/quote/direct.rs
//! Что делает: котировка «прямого» режима (решение 29) — лучший одиночный пул монеты с USDT по её реальной
//! ликвидности (локальная математика пула) с учётом налогов монеты и влияния крючка; затраты — в quote/costs.rs.
//! Покупка Q монет: сколько USDT отдать, чтобы после налога покупки и крючка пришло Q (точный выход из пула);
//! продажа Q монет: сколько USDT придёт (в пул доходит Q за вычетом налога продажи, точный вход).
//! Не хватает ликвидности в прочитанных пределах — fillable_qty меньше qty.
//! Кто вызывает: quote/mod.rs ← mod.rs (контракт quote), quote/smart.rs (расчёт доли в пуле), exec/trade*.rs.
//! От чего зависит: quote/select.rs, quote/pool_quote.rs, quote/costs.rs, check/state.rs, pools/registry.rs, tokens.rs, num.rs.
//! Настройки: нет своих; через вызываемые — см. их шапки.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::Swapped;
use super::super::num;
use super::super::pools::registry::Entry;
use super::Quoting;
use super::pool_quote;
use super::select::{self, Usable};

/// Расчёт по одному пулу.
pub struct Candidate {
    pub entry: Entry,
    pub usable: Usable,
    pub swapped: Swapped,
    /// Сколько монет реально получим (покупка) или продадим (продажа) с учётом налогов и крючка.
    pub coins: Decimal,
    /// Сколько USDT отдадим (покупка) или получим (продажа).
    pub usdt: Decimal,
}

/// Лучший пул для стороны и количества. Ошибки: NotConfigured, CoinNotChecked, CoinCheckFailed, NoRoute (с причинами по пулам).
/// Принимает: окружение, монету, сторону, количество, пулы монеты, decimals монеты и USDT.
pub fn best(
    q: &Quoting,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    entries: Vec<Entry>,
    coin_dec: u8,
    usdt_dec: u8,
) -> GatewayResult<(Candidate, TokenTaxes)> {
    let ctx = &q.ctx;
    let taxes = taxes(q, asset)?;
    let mut best: Option<Candidate> = None;
    let mut reasons = Vec::new();
    for e in entries {
        let r = select::usable(ctx, &q.checks, &e, side, usdt_dec).and_then(|u| {
            calc(ctx, &e, &u, side, qty, &taxes, (coin_dec, usdt_dec))
                .map(|(swapped, coins, usdt)| Candidate {
                    entry: e.clone(),
                    usable: u,
                    swapped,
                    coins,
                    usdt,
                })
                .map_err(|err| err.message)
        });
        match r {
            Ok(c) => {
                if best.as_ref().is_none_or(|b| better(&c, b, side)) {
                    best = Some(c);
                }
            }
            Err(why) => reasons.push(format!("{}: {why}", e.family.title)),
        }
    }
    best.map(|c| (c, taxes)).ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            format!(
                "{}: нет допустимого пула — {}",
                asset.ticker,
                reasons.join("; ")
            ),
        )
    })
}

/// Налоги монеты из проверки. Ошибки: CoinNotChecked, CoinCheckFailed.
pub fn taxes(q: &Quoting, asset: &Asset) -> GatewayResult<TokenTaxes> {
    match q.checks.coin(&asset.ticker).map(|c| c.taxes) {
        Some(CheckState::Known(t)) => Ok(t),
        Some(CheckState::Failed { reason }) => Err(q.ctx.err(
            ErrorCode::CoinCheckFailed,
            format!("{}: {reason}", asset.ticker),
        )),
        _ => Err(q.ctx.err(
            ErrorCode::CoinNotChecked,
            format!("{}: налоги ещё не проверены — торговли нет", asset.ticker),
        )),
    }
}

/// Расчёт в одном пуле. Принимает: пул, его допуск, сторону, количество монет, налоги, (decimals монеты, USDT).
/// Возвращает: (обмен в пуле, монет получим/продадим, USDT отдадим/получим).
/// Ошибки: NoRoute (математика, налоги забирают всё), BadResponse (числа не помещаются).
pub fn calc(
    ctx: &Ctx,
    e: &Entry,
    u: &Usable,
    side: Side,
    qty: Decimal,
    t: &TokenTaxes,
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<(Swapped, Decimal, Decimal)> {
    let keep = |pct: Decimal| Decimal::ONE - pct / Decimal::ONE_HUNDRED;
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    };
    match side {
        Side::Buy => {
            let k = keep(t.buy_pct) * keep(u.hook_pct);
            if k <= Decimal::ZERO {
                return Err(ctx.err(ErrorCode::NoRoute, "налог покупки и крючок забирают всё"));
            }
            let pool_out = ceil_units(qty / k, coin_dec).ok_or_else(bad)?;
            let s = pool_quote::swap(ctx, e, e.usdt, false, pool_out)?;
            let got = num::to_decimal(s.amount_out, coin_dec).ok_or_else(bad)? * k;
            let usdt = num::to_decimal(s.amount_in, usdt_dec).ok_or_else(bad)?;
            Ok((s, got.min(qty), usdt))
        }
        Side::Sell => {
            let k = keep(t.sell_pct);
            if k <= Decimal::ZERO {
                return Err(ctx.err(ErrorCode::NoRoute, "налог продажи забирает всё"));
            }
            let into_pool = num::to_units(qty * k, coin_dec).ok_or_else(bad)?;
            let s = pool_quote::swap(ctx, e, e.coin, true, into_pool)?;
            let used = num::to_decimal(s.amount_in, coin_dec).ok_or_else(bad)?;
            let sold = if s.complete { qty } else { used / k };
            let out = num::to_decimal(s.amount_out, usdt_dec).ok_or_else(bad)? * keep(u.hook_pct);
            Ok((s, sold.min(qty), out))
        }
    }
}

/// Лучше ли кандидат a, чем b: сначала полнота, затем цена (покупка — дешевле, продажа — дороже за монету).
fn better(a: &Candidate, b: &Candidate, side: Side) -> bool {
    if a.swapped.complete != b.swapped.complete {
        return a.swapped.complete;
    }
    if a.coins.is_zero() || b.coins.is_zero() {
        return a.coins > b.coins;
    }
    let (pa, pb) = (a.usdt / a.coins, b.usdt / b.coins);
    match side {
        Side::Buy => pa < pb,
        Side::Sell => pa > pb,
    }
}

/// Decimal → минимальные единицы с округлением вверх. None — не помещается.
fn ceil_units(v: Decimal, decimals: u8) -> Option<U256> {
    let u = num::to_units(v, decimals)?;
    if num::to_decimal(u, decimals)? < v {
        Some(u + U256::from(1u8))
    } else {
        Some(u)
    }
}
