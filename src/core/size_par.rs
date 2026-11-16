//! Файл: core/size_par.rs
//! Что делает: решение 336 (задача t157) — параллельные части подбора объёма (решение 66), общие для всех направлений
//! (биржа → DEX, DEX → биржа, биржа ↔ биржа, цикл DEX ↔ DEX, расчёт «без потолка», «максимальный объём» карточки):
//! - par_settings — необязательные size.parallel_points и size.bisect_probes (не заданы — 1: как до 336);
//! - probes — k равномерных проб внутри отрезка границы (k-ичный поиск, size_bounds.rs);
//! - refine_all — уточнение ВСЕХ локальных максимумов разом: тот же тернарный поиск у каждого пика (те же точки m1, m2
//!   на каждом шаге, тот же конец), но шаги всех пиков идут в ногу и точки шага считаются одной пачкой
//!   (size_memo.rs at_many). Пики не зависят друг от друга (отрезки соседних точек, общий у них — только конец), поэтому
//!   набор посчитанных точек и итог те же, что у уточнения по очереди, пока не исчерпан size.max_evals (исчерпан —
//!   при очереди недосчитан последний пик, в ногу — последние шаги всех; итог — лучший из посчитанных в обоих случаях).
//!
//! Кто вызывает: core/size_search.rs (refine_all), core/size_bounds.rs (probes), core/size_curve.rs и
//! core/cycle_plan.rs (par_settings).
//! От чего зависит: core/size.rs, core/size_memo.rs, settings.
//! Настройки: size.parallel_points, size.bisect_probes (вся система; необязательные).

use crate::settings::read::At;
use crate::settings::store::Store;

use super::size::{Curve, Fail};
use super::size_memo::Memo;

/// Что делает: (size.parallel_points, size.bisect_probes). Принимает: настройки. Возвращает: значения, не заданные или
/// меньше 1 — 1 (как до 336). Ошибки: текст — значение не помещается в u32.
pub fn par_settings(s: &Store) -> Result<(u32, u32), String> {
    let one = |name: &str| -> Result<u32, String> {
        match s.integer(name, At::default()) {
            Err(_) => Ok(1),
            Ok(v) => u32::try_from(v)
                .map(|v| v.max(1))
                .map_err(|_| format!("{name} = {v}: вне пределов")),
        }
    };
    Ok((one("size.parallel_points")?, one("size.bisect_probes")?))
}

/// Что делает: k проб внутри (lo, hi) — равномерно, по возрастанию, без повторов и без концов. Отрезок уже k + 1 —
/// все точки внутри (поиск сходится за этот шаг). Принимает: концы, k ≥ 1. Возвращает: пробы. Ошибок нет.
pub fn probes(lo: u64, hi: u64, k: u32) -> Vec<u64> {
    let span = hi.saturating_sub(lo);
    let k = u64::from(k.max(1));
    let mut out: Vec<u64> = (1..=k)
        .map(|j| lo + (u128::from(span) * u128::from(j) / u128::from(k + 1)) as u64)
        .filter(|n| *n > lo && *n < hi)
        .collect();
    out.dedup();
    out
}

/// Тернарный поиск одного пика: текущий отрезок и сколько шагов осталось.
struct Tern {
    lo: u64,
    hi: u64,
    left: u32,
}

impl Tern {
    /// Точки шага (как size_search.rs refine). Ошибок нет.
    fn thirds(&self) -> (u64, u64) {
        (
            self.lo + (self.hi - self.lo) / 3,
            self.hi - (self.hi - self.lo) / 3,
        )
    }
}

/// Что делает: уточнение всех пиков в ногу (см. шапку). Принимает: расчёты, отрезки пиков [a, b], предел шагов
/// size.refine_steps, ширину остановки. Возвращает: (). Ошибки: Stop — расчёт невозможен (первый по порядку пиков).
pub async fn refine_all<C: Curve>(
    m: &mut Memo<'_, C>,
    spans: &[(u64, u64)],
    steps: u32,
    tol: u64,
) -> Result<(), Fail> {
    let mut st: Vec<Tern> = spans
        .iter()
        .map(|&(lo, hi)| Tern {
            lo,
            hi,
            left: steps,
        })
        .collect();
    loop {
        let active: Vec<usize> = (0..st.len())
            .filter(|&i| st[i].hi - st[i].lo > tol.max(2) && st[i].left > 0)
            .collect();
        if active.is_empty() {
            break;
        }
        let mut pts = Vec::with_capacity(active.len() * 2);
        for &i in &active {
            let (m1, m2) = st[i].thirds();
            pts.push(m1);
            pts.push(m2);
        }
        m.at_many(&pts).await;
        for &i in &active {
            st[i].left -= 1;
            let (m1, m2) = st[i].thirds();
            let (f1, f2) = (m.profit(m1).await?, m.profit(m2).await?);
            if f1 < f2 {
                st[i].lo = m1;
            } else {
                st[i].hi = m2;
            }
        }
    }
    // Конец, как у refine: узкий отрезок — все его точки, иначе середина.
    let mut fin = Vec::new();
    for t in &st {
        if t.hi - t.lo <= 2 {
            fin.extend(t.lo..=t.hi);
        } else {
            fin.push(t.lo + (t.hi - t.lo) / 2);
        }
    }
    m.at_many(&fin).await;
    for n in fin {
        m.profit(n).await?;
    }
    Ok(())
}
