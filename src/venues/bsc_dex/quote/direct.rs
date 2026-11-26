//! Файл: venues/bsc_dex/quote/direct.rs
//! Что делает: котировка «прямого» режима (решение 29) — лучший одиночный пул монеты с USDT по её реальной
//! ликвидности (локальная математика пула) с учётом налога монеты ИМЕННО в этом пуле (решение 146, Usable.tax_pct) и
//! влияния крючка; затраты — в quote/costs.rs.
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
#[derive(Clone)]
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
        let r = select::usable(q, &e, side, usdt_dec).and_then(|u| {
            calc_u(ctx, &e, &u, side, qty, (coin_dec, usdt_dec)).map_err(|err| err.message)
        });
        match r {
            Ok((swapped, coins, usdt, u)) => {
                // Решение 410 (t195): запись пула переходит в кандидата (цикл владеет `entries`) — раньше копия.
                let c = Candidate {
                    entry: e,
                    usable: u,
                    swapped,
                    coins,
                    usdt,
                };
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
    // Решение 211: налоги — по адресу контракта монеты.
    match q.checks.coin_of(asset).map(|c| c.taxes) {
        Some(CheckState::Known(t)) => Ok(t),
        // Решение 247, п. В: продажа выхода (монета уже куплена) — проверка «не прошла» не закрывает продажу: модель
        // без налога, реальный выход меряет проба перед отправкой (откат — ошибка с числами; у исполнителя —
        // пере-замер). Честного отказа продажи (ханипот) проба не пропустит.
        Some(CheckState::Failed { .. }) if crate::gateway::exit_scope::is_exit() => {
            Ok(TokenTaxes {
                buy_pct: Decimal::ZERO,
                sell_pct: Decimal::ZERO,
                transfer_pct: Decimal::ZERO,
            })
        }
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

/// Расчёт в одном пуле. Принимает: пул, его допуск (налог монеты этой стороны в этом пуле — Usable.tax_pct, решение
/// 146; крючок), сторону, количество монет, (decimals монеты, USDT).
/// Возвращает: (обмен в пуле, монет получим/продадим, USDT отдадим/получим).
/// Ошибки: NoRoute (математика, налоги забирают всё, влияние крючка на этой сумме не измерено или выше лимита),
/// BadResponse (числа не помещаются).
pub fn calc(
    ctx: &Ctx,
    e: &Entry,
    u: &Usable,
    side: Side,
    qty: Decimal,
    decs: (u8, u8),
) -> GatewayResult<(Swapped, Decimal, Decimal)> {
    calc_u(ctx, e, u, side, qty, decs).map(|(s, c, d, _)| (s, c, d))
}

/// Как calc, плюс допуск пула с влиянием крючка, применённым на входе этого расчёта (решение «335 (лесенка
/// влияния)», quote/hook_curve.rs): покупка (точный выход) — settle, продажа (точный вход) — pct_at на входе монет.
/// Ошибки: как calc.
pub fn calc_u(
    ctx: &Ctx,
    e: &Entry,
    u: &Usable,
    side: Side,
    qty: Decimal,
    (coin_dec, usdt_dec): (u8, u8),
) -> GatewayResult<(Swapped, Decimal, Decimal, Usable)> {
    let keep = |pct: Decimal| Decimal::ONE - pct / Decimal::ONE_HUNDRED;
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    };
    let no = |why: String| ctx.err(ErrorCode::NoRoute, why);
    let mut applied = u.clone();
    match side {
        Side::Buy => {
            let (pct, (s, k)) = u.hook.settle(
                u.hook_pct,
                |pct| {
                    let k = keep(u.tax_pct) * keep(pct);
                    if k <= Decimal::ZERO {
                        return Err(no("налог покупки и крючок забирают всё".into()));
                    }
                    let pool_out = ceil_units(qty / k, coin_dec).ok_or_else(bad)?;
                    let s = pool_quote::swap(ctx, e, e.usdt, false, pool_out)?;
                    Ok((s.amount_in, (s, k)))
                },
                no,
            )?;
            applied.hook_pct = pct;
            let got = num::to_decimal(s.amount_out, coin_dec).ok_or_else(bad)? * k;
            let usdt = num::to_decimal(s.amount_in, usdt_dec).ok_or_else(bad)?;
            Ok((s, got.min(qty), usdt, applied))
        }
        Side::Sell => {
            let k = keep(u.tax_pct);
            if k <= Decimal::ZERO {
                return Err(ctx.err(ErrorCode::NoRoute, "налог продажи забирает всё"));
            }
            let into_pool = num::to_units(qty * k, coin_dec).ok_or_else(bad)?;
            let pct = u.hook.pct_at(u.hook_pct, into_pool).map_err(no)?;
            applied.hook_pct = pct;
            let s = pool_quote::swap(ctx, e, e.coin, true, into_pool)?;
            let used = num::to_decimal(s.amount_in, coin_dec).ok_or_else(bad)?;
            let sold = if s.complete { qty } else { used / k };
            let out = num::to_decimal(s.amount_out, usdt_dec).ok_or_else(bad)? * keep(pct);
            Ok((s, sold.min(qty), out, applied))
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
pub fn ceil_units(v: Decimal, decimals: u8) -> Option<U256> {
    let u = num::to_units(v, decimals)?;
    if num::to_decimal(u, decimals)? < v {
        Some(u + U256::from(1u8))
    } else {
        Some(u)
    }
}
