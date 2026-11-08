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
//! Решение 236: у сделки и все прочие входы — на момент решения (Frozen.decided, снят вместе со стаканами): контекст
//! направления с σ и T плана сделки, курсы затрат и затраты DEX-стороны из плана; входов нет — расчёта нет. Объём,
//! на котором вошли (Battle.qty), считается всегда, вне uncapped.max_evals, и он — «на нашем бюджете» у сделки.
//! Сборка записи (упущено, что ограничило) и таблица потолков — uncapped_record.rs (общие с пересчётом старых записей
//! по окнам, решение 218).
//! Кто вызывает: core/uncapped_queue.rs (в потоке фоновой задачи).
//! От чего зависит: frozen.rs, dirctx.rs, size_curve.rs, size_uncapped.rs, uncapped_record.rs, limits_parts.rs, dry.rs,
//! card_max.rs (passes), settings.
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
use super::size_uncapped::uncapped;
use super::uncapped_record::{Inputs, ladder_in, record};
use super::uncapped_types::{BASIS_DECISION, BASIS_LIVE, Battle, Record};

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
    // Решение 236: сделка — контекст момента решения (σ и T плана); кандидат — собирается сейчас.
    let built;
    let cx: &DirCtx = match fz {
        Some(f) => {
            &f.decided
                .as_ref()
                .ok_or("не посчитано: входы расчёта на момент решения не сняты")?
                .cx
        }
        None => {
            built = DirCtx::build(core, d)?;
            &built
        }
    };
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
    let mut p = params(core, cx, search_cap, price, fz)?;
    p.max_evals = u32::try_from(max_evals).map_err(|_| "uncapped.max_evals вне пределов")?;
    let mut curve = CoreCurve { core, cx, src: fz };
    // Решение 236: у сделки объём входа — точка «на нашем бюджете» (всегда посчитана).
    let own = (battle.verdict == "deal" && battle.qty > Decimal::ZERO).then_some(battle.qty);
    let u = uncapped(
        &mut curve,
        &p,
        budget,
        &ladder,
        |pt| passes(&pt.data, min_usd, min_pct),
        own,
    )
    .await
    .map_err(|e| e.to_string())?;
    let inputs = Inputs {
        battle,
        parts,
        limits_error,
        budget,
        min_usd,
        min_pct,
        search_cap,
        ms: t0.elapsed().as_secs_f64() * 1000.0,
        basis: if fz.is_some() {
            BASIS_DECISION
        } else {
            BASIS_LIVE
        }
        .into(),
        decided_at: fz.map(|f| f.at),
        snapshot: fz.map(Frozen::describe),
    };
    Ok(record(inputs, u, Utc::now()))
}

/// Значения таблицы потолков (uncapped.ladder_usdt — список чисел USDT; uncapped_record.rs ladder_in).
/// Ошибки: не задана, не число, не больше 0.
pub fn ladder(core: &Core) -> Result<Vec<Decimal>, String> {
    ladder_in(&core.inner.deps.settings)
}
