//! Файл: core/vol.rs
//! Что делает: волатильность цены монеты на площадке σ (за секунду) «по нашим записям» (ТЗ, шаг 4)
//! по окну последних N минут (решение 49, настройка profit.sigma_window_min):
//! σ² = Σ(ln pᵢ/pᵢ₋₁)² / Σ Δt по замерам внутри окна. Замеры старше окна удаляются — память ограничена окном.
//! Окно не задано — замеры не копятся (хранится только последняя цена), σ нет — сделки нет.
//! Решение 118 (Я-6): ряд — по направлению: (площадка, монета, сторона, площадка-напарник). Объём сравнения у каждого
//! направления свой, и замеры продажи на MEXC от BingX → MEXC и от DEX → MEXC в одном ряду выглядели бы движением цены.
//! Кто вызывает: core/evaluate.rs (замеры цен сигнала), core/dirctx.rs (σ на B).
//! От чего зависит: rust_decimal (ln, sqrt), chrono.
//! Настройки: profit.sigma_window_min (окно передаёт вызывающий).

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{Asset, Side, VenueId};

/// Ряд цен: последняя цена и шаги внутри окна (время шага, квадрат лог-доходности, Δt в секундах).
#[derive(Default, Clone)]
struct Series {
    last: Option<(Decimal, DateTime<Utc>)>,
    steps: VecDeque<(DateTime<Utc>, Decimal, Decimal)>,
}

impl Series {
    /// Удаляет шаги старше начала окна.
    fn prune(&mut self, from: DateTime<Utc>) {
        while self.steps.front().is_some_and(|(t, _, _)| *t < from) {
            self.steps.pop_front();
        }
    }
}

/// Ключ ряда: площадка, монета, продажа (true) или покупка (false) — у Side нет Hash в стыке, площадка-напарник
/// направления (решение 118).
type Key = (VenueId, Asset, bool, VenueId);

/// Волатильность по площадке, монете, стороне и напарнику (цена покупки и продажи — разные ряды).
#[derive(Default)]
pub struct Volatility {
    map: HashMap<Key, Series>,
}

impl Volatility {
    /// Добавляет замер цены. Принимает: площадку, монету, сторону, площадку-напарника направления, цену, время, окно в
    /// минутах (None — не задано: шаги не копятся). Ошибок нет: неположительная цена или время не вперёд — пропуск.
    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &mut self,
        venue: &VenueId,
        asset: &Asset,
        side: Side,
        partner: &VenueId,
        p: Decimal,
        at: DateTime<Utc>,
        window_min: Option<u64>,
    ) {
        if p <= Decimal::ZERO {
            return;
        }
        let s = self
            .map
            .entry((
                venue.clone(),
                asset.clone(),
                side == Side::Sell,
                partner.clone(),
            ))
            .or_default();
        match (s.last, window_min) {
            (Some((_, t)), _) if at <= t => return,
            (Some((prev, t)), Some(w)) => {
                let dt = Decimal::from((at - t).num_milliseconds()) / Decimal::ONE_THOUSAND;
                let r = (p / prev).ln();
                s.steps.push_back((at, r * r, dt));
                s.prune(at - Duration::minutes(w as i64));
            }
            (_, None) => s.steps.clear(),
            _ => {}
        }
        s.last = Some((p, at));
    }

    /// σ за секунду (доля, не %) по окну последних window_min минут. Принимает: площадку, монету, сторону,
    /// площадку-напарника направления, окно, сейчас.
    /// Ошибка — текст: замеров в окне нет (меньше двух цен с разным временем).
    pub fn sigma(
        &mut self,
        venue: &VenueId,
        asset: &Asset,
        side: Side,
        partner: &VenueId,
        window_min: u64,
        now: DateTime<Utc>,
    ) -> Result<Decimal, String> {
        let none = || {
            format!(
                "волатильность {} на {} (направление с {}) за последние {window_min} мин ещё не измерена",
                asset.ticker, venue.0, partner.0
            )
        };
        let s = self
            .map
            .get_mut(&(
                venue.clone(),
                asset.clone(),
                side == Side::Sell,
                partner.clone(),
            ))
            .ok_or_else(none)?;
        s.prune(now - Duration::minutes(window_min as i64));
        let (sq, dt) = s
            .steps
            .iter()
            .fold((Decimal::ZERO, Decimal::ZERO), |(a, b), (_, r, d)| {
                (a + r, b + d)
            });
        if dt <= Decimal::ZERO {
            return Err(none());
        }
        (sq / dt)
            .sqrt()
            .ok_or_else(|| "волатильность не вычисляется".to_string())
    }
}
