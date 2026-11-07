//! Файл: core/uncapped.rs
//! Что делает: расчёт «без потолка» одного направления (решение 141) — «если бы денег было
//! немерено». Сняты ВСЕ денежные ограничения: money.max_per_trade_usd, остаток лимита на монету, остаток лимита в пути,
//! свободный остаток площадки покупки (подбор под остаток, refit.rs), виртуальный бюджет сухого режима. Остаются только
//! ликвидность (стакан по уровням, пулы DEX — Fail::TooLarge котировок) и предел поиска card.max_search_usdt.
//! Модель затрат — та же кривая, что у боевого подбора объёма (size_curve.rs → costs::evaluate: торговые комиссии,
//! комиссия пула, газ свопов и approve, налог токена, вывод/депозит, запас на движение цены за перевод, проскальзывание
//! по стакану и пулу, возврат USDT); пороги — money.min_profit_usd и money.min_profit_pct, как в profit.rs. Поиск —
//! size_uncapped.rs (вся шкала, все пики). «При нашем потолке» считается ТОЙ ЖЕ моделью на боевом бюджете — упущено не
//! искажается разницей финальной котировки (боевая финальная прибыль хранится отдельно, Battle.profit).
//! Упущено — uncapped_types::missed: прошёл / сделка — (прибыль без потолка, если проходит пороги, иначе 0) − (то же на боевом бюджете), не
//! меньше 0; отказ — прибыль без потолка, если она проходит пороги, иначе 0 (в бою взяли 0).
//! Решение 217: сделка считается по снимку рынка момента решения (core/frozen.rs, взят перед ордером покупки): котировки,
//! худшие цены и изломы обеих сторон — по снимку, а не по живым данным, где наша покупка уже съела стакан или сдвинула
//! пул (живыми остаются σ, время в пути, правила пар, комиссии вывода, курсы затрат и газ — их наша сделка не двигает).
//! Снимка нет — расчёта нет (причина), живые данные не подставляются. Кандидат — на живых данных (сделки не было).
//! Кто вызывает: core/uncapped_queue.rs (в потоке фоновой задачи).
//! От чего зависит: frozen.rs, dirctx.rs, size_curve.rs, size_uncapped.rs, limits_parts.rs, dry.rs, card_max.rs (passes), settings.
//! Настройки: card.max_search_usdt (предел поиска), uncapped.max_evals (предел расчётов), uncapped.ladder_usdt (таблица
//! потолков), money.min_profit_usd, money.min_profit_pct, size.* (через size_curve::params).

use std::time::Instant;

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Direction, Side};
use crate::settings::read::At as SAt;

use super::card_max::passes;
use super::dirctx::DirCtx;
use super::engine::{Core, rd};
use super::frozen::Frozen;
use super::size_curve::{CoreCurve, params};
use super::size_uncapped::{At, uncapped};
use super::uncapped_types::{BASIS_LIVE, BASIS_SNAPSHOT, Battle, Episode, Record, missed};

/// Что делает: расчёт без потолка (см. шапку). Принимает: ядро, направление, бой, источник котировок (решение 217:
/// None — живые, кандидат; Some(Ok) — снимок рынка момента решения, сделка; Some(Err) — снимка нет).
/// Возвращает: Record (эпизод — из одного расчёта). Ошибки: текст — нет снимка, не задана настройка, контекст
/// направления, нет цены, расчёт невозможен.
pub async fn compute(
    core: &Core,
    d: &Direction,
    battle: Battle,
    src: Option<Result<Frozen, String>>,
) -> Result<Record, String> {
    let t0 = Instant::now();
    // Решение 217: снимка нет — не считаем (живые данные после нашей покупки занизили бы потенциал).
    let frozen = src
        .transpose()
        .map_err(|e| format!("не посчитано: нет снимка рынка на момент решения — {e}"))?;
    let fz = frozen.as_ref();
    let s = &core.inner.deps.settings;
    let dec = |name: &str| s.decimal(name, SAt::default()).map_err(|e| e.message);
    let search_cap = dec("card.max_search_usdt")?;
    let (min_usd, min_pct) = (dec("money.min_profit_usd")?, dec("money.min_profit_pct")?);
    let max_evals = s
        .integer("uncapped.max_evals", SAt::default())
        .map_err(|e| e.message)?;
    let ladder = ladder(core)?;
    // Составляющие бюджета — на момент решения (Battle.parts); нет — читаются сейчас.
    let (parts, limits_error) = match battle.parts {
        Some(p) => (Some(p), None),
        None => match super::limits_parts::parts(core, d, &super::dry::mode(s)) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        },
    };
    let budget = battle
        .budget
        .or(parts.map(|p| p.cap()))
        .filter(|b| *b > Decimal::ZERO)
        .ok_or(
            "нет боевого бюджета: ни подбора объёма в расчёте, ни прочитанных лимитов money.*",
        )?;
    let cx = DirCtx::build(core, d)?;
    // Решение 211 (ревью В6): стакан A — по тикеру монеты на A (venue_ticker::ticker_on).
    let on_a = super::venue_ticker::ticker_on(core, &d.asset, &d.buy_on);
    // Решение 217: со снимком — лучший ask стакана снимка (у DEX — цена боя), без живого стакана.
    let best = match fz {
        Some(f) => f.book(false).and_then(|b| b.asks.first()).map(|l| l.price),
        None => rd(&core.inner.market).best(&d.buy_on, &on_a, Side::Buy),
    };
    let price = best
        .filter(|p| !p.is_zero())
        .or((battle.qty > Decimal::ZERO).then(|| battle.invested / battle.qty))
        .filter(|p| !p.is_zero())
        .ok_or("нет цены покупки на A")?;
    let mut p = params(core, &cx, search_cap, price, fz)?;
    p.max_evals = u32::try_from(max_evals).map_err(|_| "uncapped.max_evals вне пределов")?;
    let mut curve = CoreCurve {
        core,
        cx: &cx,
        src: fz,
    };
    let u = uncapped(&mut curve, &p, budget, &ladder, |pt| {
        passes(&pt.data, min_usd, min_pct)
    })
    .await
    .map_err(|e| e.to_string())?;
    let missed = missed(&battle.verdict, u.opt_passes, u.opt.profit, &u.at_budget);
    let limited_by = limited_by(&battle, &u, parts.as_ref(), budget);
    let c = &u.opt.data;
    let now = Utc::now();
    Ok(Record {
        at: now,
        direction: d.clone(),
        battle,
        parts,
        limits_error,
        budget,
        limited_by,
        min_profit_usd: min_usd,
        min_profit_pct: min_pct,
        opt: At::of(&u.opt),
        opt_passes: u.opt_passes,
        opt_profit_pct: c.profit_pct,
        opt_buy_avg: c.buy_avg,
        opt_sell_avg: c.sell_avg,
        opt_proceeds: c.proceeds,
        opt_costs_usdt: c.costs_usdt,
        opt_transit_s: c.transit_s,
        opt_costs: c.costs.clone(),
        min_plus: u.min_plus.as_ref().map(At::of),
        at_budget: u.at_budget,
        levels: u.levels,
        missed,
        upper_qty: u.upper_qty,
        search_cap,
        cap_reached: u.cap_reached,
        limit: u.limit,
        evals: u.evals,
        peaks: u.peaks,
        capped: u.capped,
        ms: t0.elapsed().as_secs_f64() * 1000.0,
        ep: Episode {
            first_at: now,
            last_at: now,
            count: 1,
            capped_count: 0,
            peak_at: now,
        },
        basis: if fz.is_some() {
            BASIS_SNAPSHOT
        } else {
            BASIS_LIVE
        }
        .into(),
        decided_at: fz.map(|f| f.at),
        snapshot: fz.map(Frozen::describe),
    })
}

/// Значения таблицы потолков (uncapped.ladder_usdt — список чисел USDT). Ошибки: не задана, не число, не больше 0.
pub fn ladder(core: &Core) -> Result<Vec<Decimal>, String> {
    let raw = core
        .inner
        .deps
        .settings
        .list("uncapped.ladder_usdt", SAt::default())
        .map_err(|e| e.message)?;
    let mut v = raw
        .iter()
        .map(|x| match x.trim().parse::<Decimal>() {
            Ok(n) if n > Decimal::ZERO => Ok(n),
            _ => Err(format!(
                "uncapped.ladder_usdt: «{x}» — нужно число USDT больше 0"
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

/// Что ограничило (коды — uncapped_types.rs Record.limited_by). Ошибок нет.
fn limited_by<T>(
    b: &Battle,
    u: &super::size_uncapped::Uncapped<T>,
    parts: Option<&super::limits_parts::Parts>,
    budget: Decimal,
) -> String {
    if u.opt.invested <= budget {
        let edge = u.opt.qty >= u.upper_qty;
        return match (edge, u.cap_reached) {
            (false, _) => "optimum",
            (true, true) => "search_cap",
            (true, false) => "liquidity",
        }
        .into();
    }
    // Сделка: свободный остаток перед покупкой меньше бюджета плана — объём подбирался под остаток (решение 103).
    if let (Some(free), Some(plan)) = (b.free, b.plan_budget)
        && free < plan
    {
        return "free".into();
    }
    parts.map_or("budget", |p| p.binding()).into()
}
