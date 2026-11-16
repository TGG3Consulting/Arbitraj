//! Файл: venues/bsc_dex/quote/select.rs
//! Что делает: можно ли использовать пул в котировке и сделке прямо сейчас — с понятной причиной, если нельзя:
//! пул достоверен; DEX и вид пула включены; группа крючка допущена (hooks.allowed_groups), влияние B/C измерено
//! и в допуске (hooks.max_impact_b_pct / _c_pct); пул сверен с квотером без расхождения (любой пул, с крючком тоже); газ стороны измерен
//! пробным обменом; налог монеты нужной стороны ИМЕННО в этом пуле измерен пробой и не больше tax.max_buy_pct /
//! tax.max_sell_pct (решение 146); глубина нужного направления в полосе цены не ниже dex.min_pool_liquidity_usd (решение 73).
//! Ревью 273: задана плановая цена газа (Quoting.plan_gas) — пул Solidly с пределом цены газа фабрики ниже неё не
//! допущен (math/solidly_fee.rs blocked), как пул на паузе.
//! Решение 311 (А): usable_with / deep_side_with — с одной подготовкой глубины (Prep) на котировку графа: настройки
//! глубины и пулы «встречный — USDT» читаются один раз на котировку, а не на каждый хоп (quote/graph_legs.rs).
//! Решения 336, 363: usable внутри подбора объёма — один раз на пул (quote/usable_reuse.rs).
//! Кто вызывает: quote/direct.rs и все сборщики ног маршрута (graph_legs, via_pools, smart_ways, cycle*, verify_decide, exec/live*).
//! От чего зависит: math/solidly_fee.rs, check/state.rs, quote/pool_tax.rs (налог пула, решение 146), pools/registry.rs, pools/depth_book.rs, tokens.rs.
//! Настройки: dex.kind, dex.pool_enabled и адреса [DEX] (families::still), dex.stable_hooks [DEX Infinity Stable], hooks.allowed_groups, hooks.max_impact_b_pct, hooks.max_impact_c_pct,
//! tax.max_buy_pct, tax.max_sell_pct (не задан порог — пул не используется с причиной «не задана настройка», как у
//! порогов ядра core/blacklist.rs: монета на DEX не торгуется),
//! dex.min_pool_liquidity_usd, через depth_book.rs — dex.depth_band_pct, dex.depth_hysteresis_pct, dex.depth_log_min_s [bsc_dex].
//!
//! Динамическая комиссия V4 (fee = DYNAMIC_FEE_FLAG) — группа B: её влияние измеряется квотером
//! (check/hooks_probe.rs), так техлид подтвердил в Issue #7.
//! Решения 30–32: пока проверка не закончена — торговли нет; не допущенный крючок — пул исключается.
//! Глубина по направлениям — quote/depth_band.rs (та же математика, что котировка), вердикт с гистерезисом —
//! pools/depth_book.rs.

use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, PoolState, Side};
use crate::settings::read::At;

use super::super::check::state::PoolFacts;
use super::super::ctx::Ctx;
use super::super::math::solidly_fee;
use super::super::pools::depth_key::Prep;
use super::super::pools::families::{self, Proto};
use super::super::pools::registry::{Entry, Registry};
use super::super::pools::verify_inf_stable;
use super::super::tokens::Tokens;
use super::Quoting;
use super::hook_curve::HookCurve;

/// Что известно о допущенном пуле для котировки.
#[derive(Debug, Clone)]
pub struct Usable {
    /// Газ обмена этой стороны (единиц).
    pub gas: u64,
    /// Влияние крючка на эту сторону, % (0 — крючка B/C нет). Решение «335 (лесенка влияния)»: у Usable из select —
    /// наибольшее по лесенке стороны (верхняя оценка там, где сумма неизвестна); у шага пути (quote/path.rs Hop) —
    /// влияние, применённое на его входе (quote/direct.rs, quote/route_calc.rs: hook.pct_at / hook.settle).
    pub hook_pct: Decimal,
    /// Налог монеты пула (Entry.coin) на эту сторону ИМЕННО в этом пуле, % (решение 146, PoolFacts.taxes): покупка —
    /// налог покупки, продажа — налог продажи. Котировка берёт его на шаге монеты (quote/route_calc.rs coin_keep,
    /// quote/steps.rs, quote/direct.rs calc).
    pub tax_pct: Decimal,
    /// Решение «335 (лесенка влияния)»: влияние крючка как функция входа в пул (quote/hook_curve.rs); пусто — одно
    /// число hook_pct на все суммы.
    pub hook: HookCurve,
}

/// Проверка пула. Возвращает: Ok(Usable) или Err(причина, почему пул не используется).
/// Решения 336, 363: внутри подбора объёма — из короба подбора по поколению состояния пула и настроек
/// (quote/usable_reuse.rs); вне подбора — usable_with, как раньше.
pub fn usable(q: &Quoting, e: &Entry, side: Side, usdt_decimals: u8) -> Result<Usable, String> {
    super::usable_reuse::usable(q, e, side, usdt_decimals)
}

/// Решение 311 (А): то же, что usable, с готовой подготовкой глубины (pools/depth_key.rs Prep) — одной на котировку
/// графа для всех хопов всех путей (quote/graph_legs.rs legs_prep). None — подготовка на этот пул заново, как usable.
/// Принимает и возвращает: как usable. Ошибки: как usable.
pub fn usable_with(
    q: &Quoting,
    e: &Entry,
    side: Side,
    usdt_decimals: u8,
    prep: Option<&Prep>,
) -> Result<Usable, String> {
    let (ctx, checks) = (&q.ctx, &q.checks);
    if !e.valid {
        return Err("данные пула недостоверны (обрыв или перечитывание)".into());
    }
    // Пустой пул — по живому состоянию, не дожидаясь сверки с квотером (quote/emptiness.rs).
    if super::emptiness::empty(e) {
        return Err(super::emptiness::EMPTY.into());
    }
    // Ревью 273: предел цены газа фабрики пары Solidly (usdfi maxGasPrice) ниже плановой цены газа выбора маршрута —
    // обмен откатился бы; пул исключается при выборе, как пул на паузе (а не после выбора маршрута).
    if let (Some(price), PoolState::Solidly { gate, .. }) = (q.plan_gas, &e.pool.state)
        && let Some(why) = solidly_fee::blocked(gate, Some(price))
    {
        return Err(why);
    }
    let need = Need::Side(side);
    if let Some(why) = deep_side_with(ctx, &q.reg, &q.tokens, e, usdt_decimals, need, prep)? {
        return Err(why);
    }
    // DEX подключена и её вид не менялся на лету (pools/families.rs); пул не исключён проверкой «кто меняет».
    families::still(ctx, e.family)?;
    // Решение «122 (Infinity Stable, ответы)»: пул Infinity CL с крючком из dex.stable_hooks — это пул Infinity Stable
    // (одна математика на пул), видом CL в котировку не идёт (например, взят до того, как крючок внесли в список).
    if e.family.proto == Proto::InfinityCl
        && let Some(k) = e.key.as_ref()
        && verify_inf_stable::claimed(ctx, k.hooks())
    {
        return Err(verify_inf_stable::claimed_reason(k.hooks()));
    }
    if let Some(why) = ctx.index.banned(&e.pool.reference) {
        return Err(why);
    }
    let facts: PoolFacts = checks.pool(&e.pool.reference);
    let (hook_pct, hook) = hook(ctx, e, &facts, side)?;
    // Сверка с квотером обязательна для всех пулов, с крючком тоже (check/quoter_cycle.rs), — своей стороны
    // (решение 73: стороны сверяются независимо).
    match facts.quoter.side(side) {
        Some(Ok(())) => {}
        Some(Err(why)) => return Err(why.clone()),
        None => return Err("сторона пула ещё не сверена с квотером".into()),
    }
    let gas = match side {
        Side::Buy => facts.gas_buy,
        Side::Sell => facts.gas_sell,
    }
    .ok_or("газ обмена не измерен пробным обменом")?;
    let tax_pct = super::pool_tax::usable(ctx, &facts, side)?;
    Ok(Usable {
        gas,
        hook_pct,
        tax_pct,
        hook,
    })
}

/// Допуск крючка и его влияние на сторону: (наибольшее по лесенке, %, и лесенка с лимитом — решение «335 (лесенка
/// влияния)»). Err — пул не допущен (причина).
fn hook(
    ctx: &Ctx,
    e: &Entry,
    facts: &PoolFacts,
    side: Side,
) -> Result<(Decimal, HookCurve), String> {
    let Some(h) = e.pool.hooks.as_ref() else {
        return Ok((Decimal::ZERO, HookCurve::default()));
    };
    if h.group == HookGroup::A {
        return Ok((Decimal::ZERO, HookCurve::default()));
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
    // Решение 247, п. В: продажа выхода — порог входа hooks.max_impact_* не применяется (влияние — в модель).
    let exit = side == Side::Sell && crate::gateway::exit_scope::is_exit();
    let limit = if exit {
        None
    } else {
        Some(
            ctx.settings
                .decimal(limit_name, At::default())
                .map_err(|e| e.message)?,
        )
    };
    let (impact, steps) = match facts.hook.as_ref().map(|c| &c.impact) {
        Some(CheckState::Known(i)) => {
            if side == Side::Buy {
                (i.buy_pct, &i.buy)
            } else {
                (i.sell_pct, &i.sell)
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
    // Решение «335 (лесенка влияния)»: лимит — к влиянию на сумме сделки (HookCurve::pct_at); здесь пул отсекается,
    // только если лимит превышен уже на наименьшей сумме (без лесенки — на единственной).
    let least = steps.iter().map(|s| s.pct).min().unwrap_or(impact);
    if let Some(limit) = limit.filter(|l| least > *l) {
        return Err(format!(
            "влияние крючка {least}% больше {limit_name} ({limit}%)"
        ));
    }
    // Ревью Б1 (в): продажа выхода — оценка вне измеренного (open); факт на её сумме — проба перед сделкой
    // (exec/hook_trade.rs), пул для продажи держимых монет не выпадает до следующего цикла крючков.
    let curve = HookCurve {
        steps: (!steps.is_empty()).then(|| std::sync::Arc::new(steps.clone())),
        limit: limit.map(|l| (limit_name, l)),
        open: exit,
    };
    Ok((impact.max(Decimal::ZERO), curve))
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
    deep_side_with(ctx, reg, tokens, e, usdt_decimals, need, None)
}

/// Решение 311 (А): то же, что deep_side, с готовой подготовкой глубины (`prep`; Book::depth = Book::prep +
/// depth_prep — тот же расчёт). None — подготовка заново, как deep_side. Принимает, возвращает, ошибки: как deep_side.
pub fn deep_side_with(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
    usdt_decimals: u8,
    need: Need,
    prep: Option<&Prep>,
) -> Result<Option<String>, String> {
    if e.asleep {
        return Ok(Some(ASLEEP.into()));
    }
    let book = reg.depth();
    let d = match prep {
        Some(p) => book.depth_prep(ctx, reg, tokens, e, usdt_decimals, p)?,
        None => book.depth(ctx, reg, tokens, e, usdt_decimals)?,
    };
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
