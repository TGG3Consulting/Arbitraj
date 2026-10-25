//! Файл: venues/bsc_dex/quote/select.rs
//! Что делает: можно ли использовать пул в котировке и сделке прямо сейчас — с понятной причиной, если нельзя:
//! пул достоверен; DEX и вид пула включены; группа крючка допущена (hooks.allowed_groups), влияние B/C измерено
//! и в допуске (hooks.max_impact_b_pct / _c_pct); пул сверен с квотером без расхождения (любой пул, с крючком тоже); газ стороны измерен
//! пробным обменом; ликвидность в USDT не ниже dex.min_pool_liquidity_usd.
//! Кто вызывает: quote/direct.rs.
//! От чего зависит: check/state.rs, pools/registry.rs, quote/pool_quote.rs, quote/rate.rs, tokens.rs, num.rs.
//! Настройки: dex.kind, dex.pool_enabled и адреса [DEX] (families::still), hooks.allowed_groups, hooks.max_impact_b_pct, hooks.max_impact_c_pct,
//! dex.min_pool_liquidity_usd [bsc_dex].
//!
//! Динамическая комиссия V4 (fee = DYNAMIC_FEE_FLAG) — группа B: её влияние измеряется квотером
//! (check/hooks_probe.rs), так техлид подтвердил в Issue #7.
//! Решения 30–32: пока проверка не закончена — торговли нет; не допущенный крючок — пул исключается.
//! Ликвидность пула в USDT — сколько USDT пул отдаёт при продаже в него монет до границы прочитанной карты
//! (тиков/бинов) или до конца резерва V2: та же математика, что и котировка.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, Side};
use crate::settings::read::At;

use super::super::check::state::PoolFacts;
use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::families;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::{Quoting, pool_quote, rate};

/// Что известно о допущенном пуле для котировки.
#[derive(Debug, Clone)]
pub struct Usable {
    /// Газ обмена этой стороны (единиц).
    pub gas: u64,
    /// Влияние крючка на эту сторону, % (0 — крючка B/C нет).
    pub hook_pct: Decimal,
}

/// Проверка пула. Возвращает: Ok(Usable) или Err(причина, почему пул не используется).
pub fn usable(q: &Quoting, e: &Entry, side: Side, usdt_decimals: u8) -> Result<Usable, String> {
    let (ctx, checks) = (&q.ctx, &q.checks);
    if !e.valid {
        return Err("данные пула недостоверны (обрыв или перечитывание)".into());
    }
    // Пустой пул — по живому состоянию, не дожидаясь сверки с квотером (quote/emptiness.rs).
    if super::emptiness::empty(e) {
        return Err(super::emptiness::EMPTY.into());
    }
    if let Some(why) = deep_enough(ctx, &q.reg, &q.tokens, e, usdt_decimals)? {
        return Err(why);
    }
    // DEX подключена и её вид не менялся на лету (pools/families.rs); пул не исключён проверкой «кто меняет».
    families::still(ctx, e.family)?;
    if let Some(why) = ctx.index.banned(&e.pool.reference) {
        return Err(why);
    }
    let facts: PoolFacts = checks.pool(&e.pool.reference);
    let hook_pct = hook(ctx, e, &facts, side)?;
    // Сверка с квотером обязательна для всех пулов, с крючком тоже (check/quoter_cycle.rs).
    match &facts.quoter {
        Some(Ok(())) => {}
        Some(Err(why)) => return Err(why.clone()),
        None => return Err("пул ещё не сверен с квотером".into()),
    }
    let gas = match side {
        Side::Buy => facts.gas_buy,
        Side::Sell => facts.gas_sell,
    }
    .ok_or("газ обмена не измерен пробным обменом")?;
    Ok(Usable { gas, hook_pct })
}

/// Допуск крючка и его влияние на сторону, %. Err — пул не допущен (причина).
fn hook(ctx: &Ctx, e: &Entry, facts: &PoolFacts, side: Side) -> Result<Decimal, String> {
    let Some(h) = e.pool.hooks.as_ref() else {
        return Ok(Decimal::ZERO);
    };
    if h.group == HookGroup::A {
        return Ok(Decimal::ZERO);
    }
    let allowed = ctx
        .settings
        .text("hooks.allowed_groups", At::default())
        .map_err(|e| e.message)?;
    let ok = match h.group {
        HookGroup::A => true,
        HookGroup::B => allowed == "A+B" || allowed == "A+B+C",
        HookGroup::C => allowed == "A+B+C",
    };
    if !ok {
        return Err(format!(
            "крючок группы {:?} не допущен (hooks.allowed_groups = {allowed})",
            h.group
        ));
    }
    let limit_name = if h.group == HookGroup::B {
        "hooks.max_impact_b_pct"
    } else {
        "hooks.max_impact_c_pct"
    };
    let limit = ctx
        .settings
        .decimal(limit_name, At::default())
        .map_err(|e| e.message)?;
    let impact = match facts.hook.as_ref().map(|c| &c.impact) {
        Some(CheckState::Known(i)) => {
            if side == Side::Buy {
                i.buy_pct
            } else {
                i.sell_pct
            }
        }
        // Не хватает ликвидности на пробу крючка — свойство пула, причина как есть (check/hooks_probe.rs SHORT).
        Some(CheckState::Failed { reason })
            if reason.starts_with(super::super::check::hooks_probe::SHORT) =>
        {
            return Err(reason.clone());
        }
        Some(CheckState::Failed { reason }) => {
            return Err(format!("проверка крючка не прошла: {reason}"));
        }
        _ => return Err("влияние крючка ещё не измерено".into()),
    };
    if impact > limit {
        return Err(format!(
            "влияние крючка {impact}% больше {limit_name} ({limit}%)"
        ));
    }
    Ok(impact.max(Decimal::ZERO))
}

/// Ликвидность пула в USDT (см. шапку); у пула с промежуточной монетой (решение 52) выход в ней пересчитывается
/// в USDT по живому пулу «промежуточная — USDT» (quote/rate.rs). None — расчёт невозможен.
pub fn liquidity_usdt(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
    usdt_decimals: u8,
) -> Option<Decimal> {
    let all = U256::from(u128::MAX);
    let r = pool_quote::swap(ctx, e, e.coin, true, all).ok()?;
    let usdt = tokens.usdt(ctx).ok()?;
    let out = rate::to_usdt(ctx, reg, usdt, e.usdt, r.amount_out).ok()?;
    num::to_decimal(out, usdt_decimals)
}

/// Пул не мельче dex.min_pool_liquidity_usd? Этот фильтр — первым: мелкие пулы не идут ни в котировки, ни в сверку
/// с квотером, ни в пробу налогов. Возвращает: Ok(None) — достаточно глубок; Ok(Some(причина)) — мельче порога;
/// Err(причина) — глубину не посчитать (настройка, расчёт). Ошибок нет.
pub fn deep_enough(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
    usdt_decimals: u8,
) -> Result<Option<String>, String> {
    if e.asleep {
        return Ok(Some(
            "пул мельче dex.min_pool_liquidity_usd — слушается только добавление ликвидности"
                .into(),
        ));
    }
    let min = ctx
        .dec("dex.min_pool_liquidity_usd")
        .map_err(|e| e.message)?;
    let depth = liquidity_usdt(ctx, reg, tokens, e, usdt_decimals)
        .ok_or("ликвидность пула не считается")?;
    Ok((depth < min)
        .then(|| format!("ликвидность {depth} USDT ниже dex.min_pool_liquidity_usd ({min})")))
}
