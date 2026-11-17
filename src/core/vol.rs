//! Файл: core/vol.rs
//! Что делает: волатильность цены монеты на площадке σ (за секунду) «по нашим записям» (ТЗ, шаг 4)
//! по окну последних N минут (решение 49, настройка profit.sigma_window_min):
//! σ² = Σ(ln pᵢ/pᵢ₋₁)² / Σ Δt по замерам внутри окна. Замеры старше окна удаляются — память ограничена окном.
//! Окно не задано — замеры не копятся (хранится только последняя цена), σ нет — сделки нет.
//! Решение 118 (Я-6): ряд — по направлению: (площадка, монета, сторона, площадка-напарник). Объём сравнения у каждого
//! направления свой, и замеры продажи на MEXC от BingX → MEXC и от DEX → MEXC в одном ряду выглядели бы движением цены.
//! Решение 240: measure — σ вместе с числом ненулевых изменений цены в окне (profit.sigma_min_changes, core/reserve.rs).
//! Решение 358 (только журнал): шаг хранит лог-доходность со знаком (квадрат — при сложении, σ та же); horizon — σ на
//! горизонте T и автокорреляция шагов по тому же окну (core/vol_horizon.rs) для сравнения со σ рядом.
//! Решение 403 (ревью t188): key / measure_key / Miss — ключ ряда и текст причины собираются вне замка рядов ядра
//! (core/sigma_plan.rs entry_shared); measure — то же с текстом, как раньше.
//! Кто вызывает: core/evaluate.rs (замеры цен сигнала), core/dirctx.rs (σ на B), core/reserve.rs (measure),
//! core/reserve_check.rs (horizon, copy_of).
//! От чего зависит: rust_decimal (ln, sqrt), chrono, core/vol_horizon.rs.
//! Настройки: profit.sigma_window_min (окно передаёт вызывающий).

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{Asset, Side, VenueId};

/// Шаг ряда: время замера, лог-доходность ln pᵢ/pᵢ₋₁ со знаком (решение 358), Δt с прошлого замера в секундах.
pub type Step = (DateTime<Utc>, Decimal, Decimal);

/// Ряд цен: последняя цена и шаги внутри окна.
#[derive(Default, Clone)]
struct Series {
    last: Option<(Decimal, DateTime<Utc>)>,
    steps: VecDeque<Step>,
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
pub type Key = (VenueId, Asset, bool, VenueId);

/// Решение 403 (ревью t188): почему σ не измерена — без текста, текст собирает вызывающий вне замка рядов (text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// Ряда нет или в окне нет шагов.
    NotMeasured,
    /// Корень не вычисляется.
    NotComputable,
}

impl Miss {
    /// Текст причины — тот же, что у measure. Принимает: площадку, монету, напарника, окно. Ошибок нет.
    pub fn text(
        self,
        venue: &VenueId,
        asset: &Asset,
        partner: &VenueId,
        window_min: u64,
    ) -> String {
        match self {
            Miss::NotMeasured => format!(
                "волатильность {} на {} (направление с {}) за последние {window_min} мин ещё не измерена",
                asset.ticker, venue.0, partner.0
            ),
            Miss::NotComputable => "волатильность не вычисляется".to_string(),
        }
    }
}

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
                s.steps.push_back((at, r, dt));
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
        self.measure(venue, asset, side, partner, window_min, now)
            .map(|(sigma, _)| sigma)
    }

    /// Решение 240: то же, что sigma, и число шагов окна с ненулевым изменением цены (ln pᵢ/pᵢ₋₁ ≠ 0).
    /// Принимает: как sigma. Возвращает: (σ за секунду, число ненулевых изменений). Ошибки: как у sigma.
    pub fn measure(
        &mut self,
        venue: &VenueId,
        asset: &Asset,
        side: Side,
        partner: &VenueId,
        window_min: u64,
        now: DateTime<Utc>,
    ) -> Result<(Decimal, usize), String> {
        let key = Volatility::key(venue, asset, side, partner);
        self.measure_key(&key, window_min, now)
            .map_err(|m| m.text(venue, asset, partner, window_min))
    }

    /// Решение 403 (ревью t188): ключ ряда — собирается вызывающим ДО замка рядов (копии строк — вне замка).
    /// Принимает: площадку, монету, сторону, напарника. Ошибок нет.
    pub fn key(venue: &VenueId, asset: &Asset, side: Side, partner: &VenueId) -> Key {
        (
            venue.clone(),
            asset.clone(),
            side == Side::Sell,
            partner.clone(),
        )
    }

    /// Решение 403 (ревью t188): то же, что measure, по готовому ключу и без текста ошибки — под замком рядов только
    /// поиск, обрезка окна и свёртка. Принимает: ключ, окно, сейчас. Возвращает: (σ за секунду, число ненулевых
    /// изменений). Ошибки: Miss.
    pub fn measure_key(
        &mut self,
        key: &Key,
        window_min: u64,
        now: DateTime<Utc>,
    ) -> Result<(Decimal, usize), Miss> {
        let s = self.map.get_mut(key).ok_or(Miss::NotMeasured)?;
        s.prune(now - Duration::minutes(window_min as i64));
        let (sq, dt) = s
            .steps
            .iter()
            .fold((Decimal::ZERO, Decimal::ZERO), |(a, b), (_, r, d)| {
                (a + r * r, b + d)
            });
        if dt <= Decimal::ZERO {
            return Err(Miss::NotMeasured);
        }
        let changes = s.steps.iter().filter(|(_, r, _)| !r.is_zero()).count();
        (sq / dt)
            .sqrt()
            .map(|sigma| (sigma, changes))
            .ok_or(Miss::NotComputable)
    }

    /// Решение 364: копия ОДНОГО ряда (площадка, монета, сторона, напарник) — для счёта сравнения σ вне замка рядов ядра
    /// (core/reserve_check.rs entry). Шаги — Copy: копирование памяти. Принимает: как measure без окна. Возвращает:
    /// Volatility только с этим рядом (ряда нет — пустую). Ошибок нет.
    pub fn copy_of(
        &self,
        venue: &VenueId,
        asset: &Asset,
        side: Side,
        partner: &VenueId,
    ) -> Volatility {
        let key = (
            venue.clone(),
            asset.clone(),
            side == Side::Sell,
            partner.clone(),
        );
        let mut map = HashMap::new();
        if let Some(s) = self.map.get(&key) {
            map.insert(key, s.clone());
        }
        Volatility { map }
    }

    /// Решение 358 (только журнал): σ на горизонте `horizon_s` и автокорреляция шагов по окну window_min
    /// (core/vol_horizon.rs). Принимает: как measure и горизонт T в секундах. Возвращает: None — ряда нет.
    /// Ошибок нет.
    #[allow(clippy::too_many_arguments)]
    pub fn horizon(
        &mut self,
        venue: &VenueId,
        asset: &Asset,
        side: Side,
        partner: &VenueId,
        window_min: u64,
        now: DateTime<Utc>,
        horizon_s: Decimal,
    ) -> Option<super::vol_horizon::Horizon> {
        let key = (
            venue.clone(),
            asset.clone(),
            side == Side::Sell,
            partner.clone(),
        );
        let s = self.map.get_mut(&key)?;
        s.prune(now - Duration::minutes(window_min as i64));
        Some(super::vol_horizon::of(&s.steps, now, horizon_s))
    }
}
