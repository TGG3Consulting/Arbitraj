//! Файл: core/vol.rs
//! Что делает: то, что ядро измеряет само «по нашим записям» (ТЗ, шаг 4) без единого параметра:
//! волатильность цены монеты на площадке σ (за секунду, по всем замерам с запуска: σ² = Σ(ln pᵢ/pᵢ₋₁)² / Σ Δt)
//! и среднее время блока BSC (по событиям пулов: Σ Δt / Σ Δблоков). Память — O(1) на монету.
//! Кто вызывает: core/tick.rs (замеры цен сигнала), core/market_state.rs (блоки пулов), costs.rs (σ, время блока).
//! От чего зависит: rust_decimal (ln, sqrt), chrono.
//! Настройки: нет.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{Asset, Side, VenueId};

/// Накопитель волатильности одного ряда цен.
#[derive(Default, Clone)]
struct Series {
    last: Option<(Decimal, DateTime<Utc>)>,
    sum_sq: Decimal,
    sum_dt_s: Decimal,
}

/// Волатильность по площадке, монете и стороне (цена покупки и продажи — разные ряды).
#[derive(Default)]
pub struct Volatility {
    /// Ключ: площадка, монета, продажа (true) или покупка (false) — у Side нет Hash в стыке.
    map: HashMap<(VenueId, Asset, bool), Series>,
}

impl Volatility {
    /// Добавляет замер цены. Принимает: площадку, монету, сторону, цену, время. Ошибок нет:
    /// неположительная цена или время не вперёд — замер пропускается.
    pub fn sample(
        &mut self,
        venue: &VenueId,
        asset: &Asset,
        side: Side,
        p: Decimal,
        at: DateTime<Utc>,
    ) {
        if p <= Decimal::ZERO {
            return;
        }
        let s = self
            .map
            .entry((venue.clone(), asset.clone(), side == Side::Sell))
            .or_default();
        if let Some((prev, t)) = s.last {
            let dt = Decimal::from((at - t).num_milliseconds()) / Decimal::ONE_THOUSAND;
            if dt <= Decimal::ZERO {
                return;
            }
            let r = (p / prev).ln();
            s.sum_sq += r * r;
            s.sum_dt_s += dt;
        }
        s.last = Some((p, at));
    }

    /// σ за секунду (доля, не %). Err — замеров ещё нет (меньше двух цен с разным временем).
    pub fn sigma(&self, venue: &VenueId, asset: &Asset, side: Side) -> Result<Decimal, String> {
        let s = self
            .map
            .get(&(venue.clone(), asset.clone(), side == Side::Sell))
            .filter(|s| s.sum_dt_s > Decimal::ZERO)
            .ok_or_else(|| {
                format!(
                    "волатильность {} на {} ещё не измерена (нужно больше замеров цены)",
                    asset.ticker, venue.0
                )
            })?;
        (s.sum_sq / s.sum_dt_s)
            .sqrt()
            .ok_or_else(|| "волатильность не вычисляется".to_string())
    }
}

/// Часы сети BSC по событиям пулов: среднее время блока.
#[derive(Default)]
pub struct BlockClock {
    last: Option<(u64, DateTime<Utc>)>,
    sum_dt_s: Decimal,
    sum_blocks: Decimal,
}

impl BlockClock {
    /// Замер: номер блока и время получения. Учитываются только шаги вперёд. Ошибок нет.
    pub fn sample(&mut self, block: u64, at: DateTime<Utc>) {
        match self.last {
            Some((b, t)) if block > b => {
                let dt = Decimal::from((at - t).num_milliseconds()) / Decimal::ONE_THOUSAND;
                if dt > Decimal::ZERO {
                    self.sum_dt_s += dt;
                    self.sum_blocks += Decimal::from(block - b);
                }
                self.last = Some((block, at));
            }
            None => self.last = Some((block, at)),
            _ => {}
        }
    }

    /// Среднее время блока, секунды. None — ещё не измерено.
    pub fn seconds_per_block(&self) -> Option<Decimal> {
        (self.sum_blocks > Decimal::ZERO).then(|| self.sum_dt_s / self.sum_blocks)
    }
}
