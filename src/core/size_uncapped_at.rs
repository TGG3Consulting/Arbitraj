//! Файл: core/size_uncapped_at.rs
//! Что делает: части расчёта «без потолка» (решение 141, size_uncapped.rs) на готовых расчётах Memo: минимальное
//! вложение до плюса (наименьший объём, проходящий пороги) и лучшее при потолке (бисекция границы «списание ≤ потолка»
//! между соседними посчитанными точками, затем лучшая посчитанная в потолке). Предел расчётов исчерпан — бисекция
//! останавливается, итог по посчитанному.
//! Кто вызывает: core/size_uncapped.rs.
//! От чего зависит: size.rs, size_memo.rs, size_uncapped.rs (At, Level).
//! Настройки: нет (шаги бисекции — size.bisect_steps через Params).

use rust_decimal::Decimal;

use super::size::{Curve, Fail, Point};
use super::size_memo::Memo;
use super::size_uncapped::{At, Level};

/// Расчёт на n шагов, если он есть в памяти или предел ещё не исчерпан. None — не посчитать (предел).
/// Ошибки: Stop кривой.
async fn get<C: Curve>(
    m: &mut Memo<'_, C>,
    n: u64,
) -> Result<Option<Result<Point<C::Data>, Fail>>, Fail> {
    if !m.map.contains_key(&n) && m.capped() {
        return Ok(None);
    }
    match m.at(n).await {
        Err(Fail::Stop(e)) => Err(Fail::Stop(e)),
        r => Ok(Some(r)),
    }
}

/// Наименьший проходящий объём: первый проходящий среди посчитанных и бисекция вниз до предыдущего посчитанного.
/// Принимает: расчёты, проверку, шаги. Возвращает: точку или None. Ошибки: Stop.
pub async fn min_pass<C, F>(
    m: &mut Memo<'_, C>,
    pass: &F,
    steps: u32,
) -> Result<Option<Point<C::Data>>, Fail>
where
    C: Curve,
    F: Fn(&Point<C::Data>) -> bool,
{
    let ok = |r: &Result<Point<C::Data>, Fail>| r.as_ref().is_ok_and(pass);
    let Some(first) = m.map.iter().find(|(_, r)| ok(r)).map(|(n, _)| *n) else {
        return Ok(None);
    };
    let mut lo = m.map.range(..first).next_back().map_or(0, |(n, _)| *n);
    let (mut hi, mut left) = (first, steps);
    while hi - lo > 1 && left > 0 {
        left -= 1;
        let mid = lo + (hi - lo) / 2;
        match get(m, mid).await? {
            None => break,
            Some(r) if ok(&r) => hi = mid,
            Some(_) => lo = mid,
        }
    }
    Ok(m.map.get(&hi).and_then(|r| r.as_ref().ok()).cloned())
}

/// Лучшее при потолке cap: бисекция границы «списание ≤ cap» между соседними посчитанными точками, затем лучшая
/// посчитанная точка со списанием ≤ cap (при равной прибыли — меньший объём). Принимает: расчёты, потолок, шаги,
/// проверку. Возвращает: Level. Ошибки: Stop.
pub async fn level<C, F>(
    m: &mut Memo<'_, C>,
    cap: Decimal,
    steps: u32,
    pass: &F,
) -> Result<Level, Fail>
where
    C: Curve,
    F: Fn(&Point<C::Data>) -> bool,
{
    let fits = |r: &Result<Point<C::Data>, Fail>| r.as_ref().is_ok_and(|p| p.invested <= cap);
    let over = |r: &Result<Point<C::Data>, Fail>| match r {
        Ok(p) => p.invested > cap,
        Err(f) => matches!(f, Fail::TooLarge(_)),
    };
    // Наибольший посчитанный объём в потолке и ближайший выше, который уже за потолком.
    let lo = m.map.iter().rev().find(|(_, r)| fits(r)).map(|(n, _)| *n);
    let hi = m
        .map
        .iter()
        .find(|(n, r)| lo.is_none_or(|l| **n > l) && over(r))
        .map(|(n, _)| *n);
    if let Some(mut hi) = hi {
        let (mut lo, mut left) = (lo.unwrap_or(0), steps);
        while hi - lo > 1 && left > 0 {
            left -= 1;
            let mid = lo + (hi - lo) / 2;
            match get(m, mid).await? {
                None => break,
                Some(r) if over(&r) => hi = mid,
                Some(_) => lo = mid,
            }
        }
    }
    let mut best: Option<&Point<C::Data>> = None;
    for p in m.map.values().flatten().filter(|p| p.invested <= cap) {
        if best.is_none_or(|b| p.profit > b.profit) {
            best = Some(p);
        }
    }
    Ok(Level {
        cap,
        at: best.map(At::of),
        passes: best.is_some_and(pass),
    })
}
