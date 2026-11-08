//! Файл: core/size_uncapped.rs
//! Что делает: расчёт «без потолка» (решение 141) на кривой прибыли — те же поиск и модель затрат, что у сделки
//! (size_search.rs, решение 66), но верхняя граница объёма — не бюджет, а предел поиска (Params.budget) и
//! ликвидность (Fail::TooLarge кривой: глубина стакана, карта пула, максимумы вывода и ордера). На одних расчётах (Memo):
//! 1) оптимум без потолка — поиск по всей шкале: границы, изломы, сетка, уточнение у КАЖДОГО локального максимума;
//! 2) край поиска — size_max::edge (предел или площадки);
//! 3) минимальное вложение «до плюса» — наименьший объём, проходящий пороги прибыли: первый проходящий среди
//!    посчитанных и бисекция вниз до предыдущего посчитанного (не больше size.bisect_steps шагов). Участок прибыльности
//!    уже шага сетки, лежащий ниже, может быть пропущен — так и пишется на экране;
//! 4) лучшее при потолке C (боевой бюджет и каждое значение таблицы потолков): граница «списание ≤ C» бисекцией между
//!    соседними посчитанными точками, затем лучшая из посчитанных точек со списанием ≤ C — локальные максимумы ниже C
//!    уже уточнены в п. 1.
//!
//! 5) решение 237: объём боя (у сделки — объём, на котором вошли) считается ПЕРВЫМ и всегда, вне предела расчётов
//!    (Memo::force); у сделки «на нашем бюджете» — именно эта точка (вложено против потолка — одна модель, одни данные),
//!    а не лучшая посчитанная в бюджете: при исчерпанном пределе поиск мог её не посчитать (сделка INTCB 08.11:
//!    вложено 450, «на бюджете» — точка 340.81).
//!
//! Предел расчётов — Params.max_evals (у ядра — uncapped.max_evals); исчерпан — берётся лучшее из посчитанного, отметка
//! capped. Ни одна точка не считается дважды.
//! Кто вызывает: core/uncapped.rs (кривая ядра), тесты (tests/core_uncapped.rs — записанные стакан и пул CAKE).
//! От чего зависит: size.rs, size_memo.rs, size_search.rs, size_max.rs (edge), size_uncapped_at.rs (минимум до плюса,
//! лучшее при потолке).
//! Настройки: через Params (size.*, предел поиска — в Params.budget, предел расчётов — Params.max_evals).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::size::{Curve, Fail, Params, Point};
use super::size_max::edge;
use super::size_memo::Memo;
use super::size_search::search_in;
use super::size_uncapped_at::{level, min_pass};

/// Объём, списание по худшей цене (USDT) и прибыль после всех затрат (USDT) одной точки.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct At {
    pub qty: Decimal,
    pub invested: Decimal,
    pub profit: Decimal,
}

impl At {
    /// Из точки кривой. Ошибок нет.
    pub fn of<T>(p: &Point<T>) -> At {
        At {
            qty: p.qty,
            invested: p.invested,
            profit: p.profit,
        }
    }
}

/// Лучшее при потолке: точка (None — ни один допустимый объём со списанием ≤ потолка), проходит ли пороги.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub cap: Decimal,
    pub at: Option<At>,
    pub passes: bool,
}

/// Итог расчёта без потолка.
#[derive(Debug, Clone)]
pub struct Uncapped<T> {
    /// Оптимум без потолка и проходит ли он пороги.
    pub opt: Point<T>,
    pub opt_passes: bool,
    /// Наименьший объём, проходящий пороги; None — ни один посчитанный не проходит («не становится плюсовой»).
    pub min_plus: Option<Point<T>>,
    /// Лучшее при боевом бюджете (та же модель) и при каждом значении таблицы потолков.
    pub at_budget: Level,
    pub levels: Vec<Level>,
    /// Наибольший возможный объём поиска; упёрлись в предел поиска (иначе — в площадки); текст края.
    pub upper_qty: Decimal,
    pub cap_reached: bool,
    pub limit: String,
    pub evals: u32,
    pub peaks: u32,
    /// Предел расчётов исчерпан — итог по посчитанному.
    pub capped: bool,
}

/// Что делает: расчёт без потолка (см. шапку). Принимает: кривую, параметры (budget = предел поиска), боевой бюджет,
/// значения таблицы потолков, проверку порогов точки, объём сделки (решение 237: Some — «на нашем бюджете» = точка на
/// этом объёме, посчитанная всегда; None — кандидат: лучшая посчитанная в бюджете). Возвращает: Uncapped.
/// Ошибки: Fail поиска (Stop — расчёт невозможен; TooSmall / TooLarge — ни один объём до предела не подходит).
pub async fn uncapped<C, F>(
    curve: &mut C,
    p: &Params,
    budget: Decimal,
    ladder: &[Decimal],
    pass: F,
    own: Option<Decimal>,
) -> Result<Uncapped<C::Data>, Fail>
where
    C: Curve,
    F: Fn(&Point<C::Data>) -> bool,
{
    if p.step <= Decimal::ZERO {
        return Err(Fail::Stop(format!(
            "шаг количества {} не больше нуля",
            p.step
        )));
    }
    let mut m = Memo::new(curve, p.step, p.budget, p.max_evals);
    // Решение 237: объём сделки — первым, вне предела расчётов (Stop — расчёт невозможен при любом объёме).
    let own_pt = match own.and_then(|q| m.units(q)) {
        Some(n) => match m.force(n).await {
            Err(Fail::Stop(e)) => return Err(Fail::Stop(e)),
            r => Some(r.ok()),
        },
        None => None,
    };
    let f = search_in(&mut m, p).await?;
    let upper = f.choice.max_qty;
    let hi = m
        .units(upper)
        .ok_or_else(|| Fail::Stop(format!("объём {upper} слишком велик")))?;
    let (cap_reached, limit) = edge(&mut m, p, hi, f.choice.max_exact).await;
    let min_plus = min_pass(&mut m, &pass, p.bisect_steps).await?;
    let at_budget = match own_pt {
        // Сделка: точка на вложенном объёме (не посчиталась — объёма нет: None, причина в записи — limit поиска).
        Some(pt) => Level {
            cap: budget,
            passes: pt.as_ref().is_some_and(&pass),
            at: pt.as_ref().map(At::of),
        },
        None => level(&mut m, budget, p.bisect_steps, &pass).await?,
    };
    let mut levels = Vec::with_capacity(ladder.len());
    for c in ladder {
        levels.push(level(&mut m, *c, p.bisect_steps, &pass).await?);
    }
    let opt = m
        .best()
        .ok_or_else(|| Fail::Stop("ни один объём не посчитан".into()))?;
    Ok(Uncapped {
        opt_passes: pass(&opt),
        opt,
        min_plus,
        at_budget,
        levels,
        upper_qty: upper,
        cap_reached,
        limit,
        evals: m.evals(),
        peaks: f.choice.peaks,
        capped: m.capped(),
    })
}
