//! Файл: core/size_batch.rs
//! Что делает: решение 336 (задача t157) — пачки независимых точек подбора объёма и короб подбора для адаптеров:
//! - with_share — сколько точек считать за раз (size.parallel_points) и короб (gateway/calc_share.rs) — только когда он
//!   нужен: точки параллельно или повторное использование (у направления с DEX — решение 363); иначе короба нет и
//!   расчёт точки — как до 336 (ревью t157 Б1);
//! - at_many — набор объёмов пачками по size.parallel_points (Curve::batch: кривая ядра считает пачку одновременно).
//!
//! Вынесено из size_memo.rs (ревью t157 Н5: файл перерос 200 строк).
//! Кто вызывает: core/size_search.rs, core/size_max.rs, core/size_uncapped.rs (with_share), core/size_bounds.rs,
//! core/size_points.rs, core/size_par.rs (at_many, par).
//! От чего зависит: core/size_memo.rs, core/size.rs, core/calc_detail.rs, gateway/calc_share.rs.
//! Настройки: нет (через Params).

use std::collections::BTreeSet;
use std::sync::Arc;

use rust_decimal::Decimal;

use crate::gateway::calc_share::{self, Share};

use super::calc_detail::stage_detail;
use super::size::{Curve, Fail};
use super::size_memo::{EVAL_BATCH, Memo, over_budget};

impl<C: Curve> Memo<'_, C> {
    /// Что делает: пачки по `par` точек и короб подбора. Принимает: par (0 и 1 — по одной), включено ли повторное
    /// использование. Возвращает: себя. Ошибок нет.
    pub fn with_share(mut self, par: u32, reuse: bool) -> Self {
        self.par = usize::try_from(par.max(1)).unwrap_or(1);
        let parallel = self.par > 1;
        self.share = Share::needed(parallel, reuse).then(|| Arc::new(Share::new(parallel, reuse)));
        self
    }

    /// Сколько точек считается за раз. Ошибок нет.
    pub fn par(&self) -> usize {
        self.par
    }

    /// Что делает: расчёт набора объёмов пачками по par (Curve::batch). Порядок — как дан; уже посчитанные, n = 0 и
    /// повторы пропускаются; сверх предела size.max_evals — не считаются (как at: такие точки — «нет в памяти»).
    /// Пачка с Fail::Stop — следующие пачки не считаются (вызывающий всё равно остановится на этой точке).
    /// Итоги — в памяти (читать at / profit), инвариант бюджета — как у at. Принимает: номера шагов. Ошибок нет.
    pub async fn at_many(&mut self, ns: &[u64]) {
        let mut seen = BTreeSet::new();
        let mut todo: Vec<u64> = ns
            .iter()
            .copied()
            .filter(|n| *n > 0 && !self.map.contains_key(n) && seen.insert(*n))
            .collect();
        todo.truncate(usize::try_from(self.max.saturating_sub(self.done)).unwrap_or(0));
        for chunk in todo.chunks(self.par.max(1)) {
            let qs: Vec<Decimal> = chunk.iter().map(|&n| self.qty(n)).collect();
            self.done += u32::try_from(chunk.len()).unwrap_or(u32::MAX);
            // Уступаем рабочий поток перед пачкой (решение 90, как at).
            tokio::task::yield_now().await;
            let rs = match self.share.clone() {
                Some(s) => {
                    let work = calc_share::scope(s, self.curve.batch(qs));
                    stage_detail(|| EVAL_BATCH.into(), work).await
                }
                None => stage_detail(|| EVAL_BATCH.into(), self.curve.batch(qs)).await,
            };
            let mut stop = false;
            for (&n, r) in chunk.iter().zip(rs) {
                let q = self.qty(n);
                let r = match r {
                    Ok(p) if p.invested > self.budget => {
                        Err(over_budget(q, p.invested, self.budget))
                    }
                    r => r,
                };
                stop |= matches!(r, Err(Fail::Stop(_)));
                self.map.insert(n, r);
            }
            if stop {
                break;
            }
        }
    }
}
