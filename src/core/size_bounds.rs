//! Файл: core/size_bounds.rs
//! Что делает: границы отрезка поиска объёма (решение 66) бисекцией по целым шагам количества:
//! - наибольший объём — самый большой, у которого вложение ≤ бюджета и хватает глубины, максимумов вывода и суммы
//!   ордера (Fail::TooLarge монотонно: больше объём — больше вложение и нужная глубина). Ищется точно, а не как
//!   «бюджет / лучшая цена»: эта оценка — только старт (у неё вложение может оказаться чуть больше бюджета из-за
//!   проскальзывания по уровням — живой случай WFI);
//! - наименьший объём — самый маленький, проходящий минимумы вывода, депозита и суммы ордера (Fail::TooSmall
//!   монотонно: меньше объём — меньше вывод, приход и сумма).
//!
//! Шагов бисекции — не больше size.bisect_steps; не хватило — берётся безопасная сторона (допустимый объём).
//! Решение 336: при size.bisect_probes = k > 1 шаг бисекции — k проб внутри отрезка одной пачкой (size_memo.rs at_many,
//! size_par.rs probes) и выбор по порядку возрастания: первая проба «за границей» — новый верх, последняя до неё — новый
//! низ. Отрезок за шаг сужается в k + 1 раз (двоичный — в 2), граница при монотонном признаке — та же (она одна); шагов
//! (size.bisect_steps) нужно меньше, расчётов — больше (k на шаг). Удвоение от оценки — по одному, как раньше
//! (обычно 1–2 шага). k = 1 — ровно как до 336.
//! Кто вызывает: core/size_search.rs.
//! От чего зависит: core/size_memo.rs, core/size.rs, core/size_par.rs.
//! Настройки: size.bisect_steps, size.bisect_probes (через Params).

use super::size::{Curve, Fail};
use super::size_memo::Memo;
use super::size_par::probes;

/// Наибольший допустимый объём в шагах. Принимает: расчёты, оценку (шагов), предел шагов, проб за шаг (решение 336).
/// Возвращает: (n, найден точно). Ошибки: Stop — расчёт невозможен; TooLarge — не подходит даже один шаг.
pub async fn upper<C: Curve>(
    m: &mut Memo<'_, C>,
    guess: u64,
    steps: u32,
    k: u32,
) -> Result<(u64, bool), Fail> {
    let mut left = steps;
    let large = |r: &Result<_, Fail>| matches!(r, Err(f) if f.too_large());
    let g = guess.max(1);
    let r = m.at(g).await;
    if let Err(Fail::Stop(e)) = r {
        return Err(Fail::Stop(e));
    }
    // (lo — не «слишком большой», hi — «слишком большой» или None, пока не найден).
    let (mut lo, mut hi) = if large(&r) { (0, Some(g)) } else { (g, None) };
    // Оценка не превысила бюджет — растём вдвое, пока не упрёмся (оценка снизу у DEX бывает неточной).
    while hi.is_none() && left > 0 {
        left -= 1;
        let n = lo.saturating_mul(2);
        match m.at(n).await {
            Err(Fail::Stop(e)) => return Err(Fail::Stop(e)),
            Err(f) if f.too_large() => hi = Some(n),
            _ => lo = n,
        }
    }
    let Some(mut hi) = hi else {
        return Ok((lo, false));
    };
    while hi - lo > 1 && left > 0 {
        left -= 1;
        // Решение 336: k проб одной пачкой; первая «слишком большая» по возрастанию — новый верх.
        if k > 1 {
            let pts = probes(lo, hi, k);
            m.at_many(&pts).await;
            for n in pts {
                match m.at(n).await {
                    Err(Fail::Stop(e)) => return Err(Fail::Stop(e)),
                    Err(f) if f.too_large() => {
                        hi = n;
                        break;
                    }
                    _ => lo = n,
                }
            }
            continue;
        }
        let mid = lo + (hi - lo) / 2;
        match m.at(mid).await {
            Err(Fail::Stop(e)) => return Err(Fail::Stop(e)),
            Err(f) if f.too_large() => hi = mid,
            _ => lo = mid,
        }
    }
    if lo == 0 {
        let why = m
            .at(hi)
            .await
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        return Err(Fail::TooLarge(if hi == 1 {
            format!("не подходит даже наименьший объём {}: {why}", m.step)
        } else {
            format!("граница бюджета не найдена за size.bisect_steps = {steps}: {why}")
        }));
    }
    Ok((lo, hi - lo == 1))
}

/// Наименьший допустимый объём в шагах на [1, hi]. Принимает: расчёты, наибольший объём, предел шагов, проб за шаг
/// (решение 336). Возвращает: n (если шагов не хватило — допустимая сторона, то есть не меньше точного минимума).
/// Ошибки: Stop; TooSmall — даже наибольший объём меньше минимумов площадок.
pub async fn lower<C: Curve>(
    m: &mut Memo<'_, C>,
    hi: u64,
    steps: u32,
    k: u32,
) -> Result<u64, Fail> {
    match m.at(hi).await {
        Err(Fail::TooSmall(e)) => {
            return Err(Fail::TooSmall(format!(
                "даже наибольший объём {} меньше минимума: {e}",
                m.qty(hi)
            )));
        }
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let (mut lo, mut hi, mut left) = (0u64, hi, steps);
    while hi - lo > 1 && left > 0 {
        left -= 1;
        // Решение 336: k проб одной пачкой; первая не «меньше минимума» по возрастанию — новый верх.
        if k > 1 {
            let pts = probes(lo, hi, k);
            m.at_many(&pts).await;
            for n in pts {
                match m.at(n).await {
                    Err(Fail::Stop(e)) => return Err(Fail::Stop(e)),
                    Err(Fail::TooSmall(_)) => lo = n,
                    _ => {
                        hi = n;
                        break;
                    }
                }
            }
            continue;
        }
        let mid = lo + (hi - lo) / 2;
        match m.at(mid).await {
            Err(Fail::Stop(e)) => return Err(Fail::Stop(e)),
            Err(Fail::TooSmall(_)) => lo = mid,
            _ => hi = mid,
        }
    }
    Ok(hi)
}
