//! Файл: venues/bsc_dex/quote/select.rs
//! Что делает: можно ли использовать пул в котировке и сделке прямо сейчас — с понятной причиной, если нельзя:
//! пул достоверен; DEX и вид пула включены; группа крючка допущена (hooks.allowed_groups), влияние B/C измерено
//! и в допуске (hooks.max_impact_b_pct / _c_pct); пул сверен с квотером без расхождения (любой пул, с крючком тоже); газ стороны измерен
//! пробным обменом; глубина нужного направления в полосе цены не ниже dex.min_pool_liquidity_usd (решение 73).
//! Кто вызывает: quote/direct.rs.
//! От чего зависит: check/state.rs, pools/registry.rs, pools/depth_book.rs, tokens.rs.
//! Настройки: dex.kind, dex.pool_enabled и адреса [DEX] (families::still), hooks.allowed_groups, hooks.max_impact_b_pct, hooks.max_impact_c_pct,
//! dex.min_pool_liquidity_usd, через depth_book.rs — dex.depth_band_pct, dex.depth_hysteresis_pct, dex.depth_log_min_s [bsc_dex].
//!
//! Динамическая комиссия V4 (fee = DYNAMIC_FEE_FLAG) — группа B: её влияние измеряется квотером
//! (check/hooks_probe.rs), так техлид подтвердил в Issue #7.
//! Решения 30–32: пока проверка не закончена — торговли нет; не допущенный крючок — пул исключается.
//! Глубина по направлениям — quote/depth_band.rs (та же математика, что котировка), вердикт с гистерезисом —
//! pools/depth_book.rs.

use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, Side};
use crate::settings::read::At;

use super::super::check::state::PoolFacts;
use super::super::ctx::Ctx;
use super::super::pools::families;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::Quoting;

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
    if let Some(why) = deep_side(ctx, &q.reg, &q.tokens, e, usdt_decimals, Need::Side(side))? {
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

/// Какое направление пула нужно (решение 73).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Одна сторона сделки: покупка монеты (USDT → монета) или продажа (монета → USDT).
    Side(Side),
    /// Оба: проба налогов покупает и продаёт тем же пулом.
    Both,
    /// Любое: источник USDT пробы, решение «слушать ли пул целиком».
    Any,
}

/// Причина «пул спит»: мелкий пул слушается только по добавлению ликвидности (pools/depth.rs).
pub const ASLEEP: &str =
    "пул мельче dex.min_pool_liquidity_usd — слушается только добавление ликвидности";

/// Достаточно ли пул глубок по нужному направлению (решение 73: глубина в полосе dex.depth_band_pct не ниже
/// dex.min_pool_liquidity_usd, с гистерезисом — pools/depth_book.rs). Этот фильтр — первым: мелкие пулы не идут ни в
/// котировки, ни в сверку с квотером, ни в пробу налогов. Спящий пул (`asleep`) — мелкий без расчёта. Возвращает:
/// Ok(None) — глубок; Ok(Some(причина с цифрами)) — мелок; Err(причина) — глубину не посчитать (настройка, расчёт).
pub fn deep_side(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
    usdt_decimals: u8,
    need: Need,
) -> Result<Option<String>, String> {
    if e.asleep {
        return Ok(Some(ASLEEP.into()));
    }
    let d = reg.depth().depth(ctx, reg, tokens, e, usdt_decimals)?;
    let buy = (d.deep_buy, "покупки", d.band.buy);
    let sell = (d.deep_sell, "продажи", d.band.sell);
    let short: Vec<_> = match need {
        Need::Side(Side::Buy) => vec![buy],
        Need::Side(Side::Sell) => vec![sell],
        Need::Both => vec![buy, sell],
        Need::Any if d.deep_buy || d.deep_sell => vec![],
        Need::Any => vec![buy, sell],
    }
    .into_iter()
    .filter(|x| !x.0)
    .collect();
    if short.is_empty() {
        return Ok(None);
    }
    let parts: Vec<String> = short
        .iter()
        .map(|(_, what, v)| format!("глубина {what} {} USDT", v.round_dp(2)))
        .collect();
    Ok(Some(format!(
        "{} в полосе ±{}% ниже dex.min_pool_liquidity_usd ({}){}",
        parts.join(", "),
        d.band_pct,
        d.min,
        if d.band.full {
            ""
        } else {
            " (полоса прочитана не целиком)"
        }
    )))
}
