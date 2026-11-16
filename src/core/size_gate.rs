//! Файл: core/size_gate.rs
//! Что делает: пороги направления в расчётах подбора объёма (решения 342 и 345, ревью t159):
//! - Gate — порог прибыли в USDT и порог % (Params.ext), passes — точка проходит оба (как финальная проверка profit.rs:
//!   Calc.profit_usd ≥ money.min_profit_usd и Calc.profit_pct ≥ money.min_profit_pct);
//! - best_pct — лучшая по USDT точка среди проходящих порог % (size_pct.rs);
//! - мягкий срок (ревью t159 Б2): arm — ставится ПОСЛЕ границ (границы никогда не обрываются сроком: иначе Fail::Stop и
//!   отказ там, где база исполнила бы сделку); late — срок наступил И среди посчитанных уже есть точка, проходящая оба
//!   порога (ревью t159 Н11: без неё подбор идёт дальше, как без срока, — финальная котировка не тратится на
//!   непроходящий объём, а найденный проходящий не теряется отказом по общему сроку).
//!
//! Кто вызывает: core/size_search.rs, core/size_memo.rs (note), core/size_batch.rs (note), core/size_pct.rs.
//! От чего зависит: core/size_memo.rs, core/size.rs.
//! Настройки: нет (пороги и срок — через Params.ext, size_extra.rs).

use std::time::Instant;

use rust_decimal::Decimal;

use super::size::{Curve, Fail, Point};
use super::size_memo::Memo;

/// Пороги направления для точек подбора. None — порога нет (не проверяется).
#[derive(Debug, Clone, Copy, Default)]
pub struct Gate {
    pub min_usd: Option<Decimal>,
    pub min_pct: Option<Decimal>,
}

impl Gate {
    /// Что делает: точка проходит порог %. Порог задан, а кривая процента не знает — не проходит. Ошибок нет.
    pub fn pct_ok<C: Curve>(&self, p: &Point<C::Data>) -> bool {
        self.min_pct
            .is_none_or(|t| C::pct(&p.data).is_some_and(|v| v >= t))
    }

    /// Что делает: точка проходит оба порога. Ошибок нет.
    pub fn passes<C: Curve>(&self, p: &Point<C::Data>) -> bool {
        self.min_usd.is_none_or(|u| p.profit >= u) && self.pct_ok::<C>(p)
    }
}

impl<C: Curve> Memo<'_, C> {
    /// Что делает: пороги направления (Params.ext). Принимает: пороги. Возвращает: себя. Ошибок нет.
    pub fn with_gate(mut self, min_usd: Option<Decimal>, min_pct: Option<Decimal>) -> Self {
        self.gate = Gate { min_usd, min_pct };
        self
    }

    /// Что делает: ревью t159 Б2 — ставит мягкий срок (после границ). Признак «проходящий посчитан» — по уже
    /// посчитанным точкам (границы). Принимает: срок (None — без срока). Ошибок нет.
    pub fn arm(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
        let gate = self.gate;
        self.passed =
            deadline.is_some() && self.map.values().flatten().any(|p| gate.passes::<C>(p));
    }

    /// Решение 345: мягкий срок наступил и проходящая оба порога точка уже есть. Ошибок нет.
    pub fn late(&self) -> bool {
        self.passed && self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Что делает: после расчёта точки — признак «проходящий посчитан» (только при сроке). Принимает: итог. Ошибок нет.
    pub(super) fn note(&mut self, r: &Result<Point<C::Data>, Fail>) {
        if self.deadline.is_some()
            && !self.passed
            && let Ok(p) = r
        {
            self.passed = self.gate.passes::<C>(p);
        }
    }

    /// Пороги подбора. Ошибок нет.
    pub fn gate(&self) -> Gate {
        self.gate
    }

    /// Решение 342: лучшая допустимая точка среди проходящих порог %: наибольшая прибыль, при равной — меньший объём.
    /// Принимает: верхний номер шага (включительно; None — все). Возвращает: точку; None — ни одна не проходит. Ошибок
    /// нет.
    pub fn best_pct(&self, upto: Option<u64>) -> Option<(u64, Point<C::Data>)> {
        let gate = self.gate;
        let mut best: Option<(u64, &Point<C::Data>)> = None;
        for (n, r) in &self.map {
            if upto.is_some_and(|u| *n > u) {
                break;
            }
            if let Ok(p) = r
                && gate.pct_ok::<C>(p)
                && best.is_none_or(|(_, b)| p.profit > b.profit)
            {
                best = Some((*n, p));
            }
        }
        best.map(|(n, p)| (n, p.clone()))
    }
}
