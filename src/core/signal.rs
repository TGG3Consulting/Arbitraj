//! Файл: core/signal.rs
//! Что делает: грубый спред по каждому направлению монеты и фильтр шума (шаг 3 ТЗ).
//! Грубый спред A→B = (цена продажи на B − цена покупки на A) / цена покупки на A, в процентах.
//! Порог перехода к полному расчёту — сумма процентных комиссий обеих сторон, которых нет в цене
//! (ТЗ, шаг 3: «ниже этого заработать нельзя ни на каком объёме»). Сигнал отбрасывается, если спред
//! мигает (держится меньше signal.min_hold_ms или меньше signal.min_hold_updates обновлений подряд),
//! аномально велик (выше signal.anomaly_spread_pct — нужна перепроверка статусов ввода/вывода)
//! или по направлению идёт пауза после сделки или ошибки (signal.route_cooldown_s).
//! Кто вызывает: core/engine.rs на каждый тик монеты (после prices.rs).
//! От чего зависит: core/types, settings.
//! Настройки: signal.min_hold_ms, signal.min_hold_updates, signal.anomaly_spread_pct, signal.route_cooldown_s.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::Direction;
use crate::settings::read::At;
use crate::settings::store::Store;

/// Пороги фильтра шума — из настроек, читаются на каждый тик (меняются на лету).
pub struct Params {
    pub hold_ms: u64,
    pub hold_updates: u64,
    pub anomaly_pct: Decimal,
}

impl Params {
    /// Читает пороги. Ошибка — текст «не задана настройка …»: сигналов нет, причина видна.
    pub fn read(s: &Store) -> Result<Self, String> {
        let at = At::default();
        Ok(Self {
            hold_ms: s.integer("signal.min_hold_ms", at).map_err(|e| e.message)?,
            hold_updates: s
                .integer("signal.min_hold_updates", at)
                .map_err(|e| e.message)?,
            anomaly_pct: s
                .decimal("signal.anomaly_spread_pct", at)
                .map_err(|e| e.message)?,
        })
    }
}

/// Цены направления на объём сравнения (prices.rs).
#[derive(Debug, Clone, Serialize)]
pub struct Prices {
    /// Средняя цена покупки на A.
    pub buy: Decimal,
    /// Средняя цена продажи на B.
    pub sell: Decimal,
    /// Сумма процентных комиссий обеих сторон, которых нет в цене (торговая комиссия биржи), %.
    pub fees_pct: Decimal,
    /// Объём сравнения, монет.
    pub qty: Decimal,
}

/// Состояние направления на «Живом экране».
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum State {
    /// Нет цены на одной из сторон — причина как есть.
    NoPrice(String),
    /// Не заданы пороги фильтра шума.
    NotConfigured(String),
    /// Спред не покрывает комиссии.
    Below,
    /// Спред выше порога, но ещё не продержался нужное время / число обновлений.
    Holding,
    /// Спред аномально велик — нужна перепроверка статусов ввода и вывода.
    Anomaly,
    /// Пауза направления после сделки или ошибки.
    Cooldown { until: DateTime<Utc> },
    /// Кандидат: прошёл порог и все фильтры — уходит в полный расчёт.
    Candidate,
    /// Налоги монеты ещё не проверены (решение 30): строка видна, торговли нет — причина ожидания.
    TaxesNotChecked(String),
}

/// Строка «Живого экрана».
#[derive(Debug, Clone, Serialize)]
pub struct SpreadView {
    pub direction: Direction,
    pub prices: Option<Prices>,
    /// Грубый спред, %.
    pub spread_pct: Option<Decimal>,
    pub state: State,
    /// С какого момента спред держится выше порога.
    pub above_since: Option<DateTime<Utc>>,
    /// Сколько обновлений подряд выше порога.
    pub updates: u64,
    pub at: DateTime<Utc>,
}

/// Память фильтра по направлению.
#[derive(Default)]
struct Track {
    above_since: Option<DateTime<Utc>>,
    updates: u64,
    cooldown_until: Option<DateTime<Utc>>,
}

/// Фильтр шума по всем направлениям.
#[derive(Default)]
pub struct Signals {
    map: HashMap<Direction, Track>,
}

impl Signals {
    /// Обновляет направление новым тиком.
    /// Принимает: направление, цены (Err — причина, почему цены нет), пороги, время.
    /// Возвращает: строку «Живого экрана». Ошибок нет.
    pub fn update(
        &mut self,
        dir: &Direction,
        prices: Result<Prices, String>,
        params: &Result<Params, String>,
        now: DateTime<Utc>,
    ) -> SpreadView {
        let t = self.map.entry(dir.clone()).or_default();
        let mut view = SpreadView {
            direction: dir.clone(),
            prices: None,
            spread_pct: None,
            state: State::Below,
            above_since: None,
            updates: 0,
            at: now,
        };
        let p = match prices {
            Ok(p) => p,
            Err(e) => {
                *t = Track {
                    cooldown_until: t.cooldown_until,
                    ..Track::default()
                };
                view.state = State::NoPrice(e);
                return view;
            }
        };
        let spread = if p.buy.is_zero() {
            Decimal::ZERO
        } else {
            (p.sell - p.buy) / p.buy * Decimal::ONE_HUNDRED
        };
        view.spread_pct = Some(spread);
        view.prices = Some(p.clone());
        if spread <= p.fees_pct {
            t.above_since = None;
            t.updates = 0;
            return view;
        }
        t.above_since.get_or_insert(now);
        t.updates += 1;
        view.above_since = t.above_since;
        view.updates = t.updates;
        view.state = match (params, t.cooldown_until) {
            (Err(e), _) => State::NotConfigured(e.clone()),
            (_, Some(until)) if until > now => State::Cooldown { until },
            (Ok(par), _) if spread > par.anomaly_pct => State::Anomaly,
            (Ok(par), _) => {
                let held = t
                    .above_since
                    .map(|s| (now - s).num_milliseconds().max(0) as u64)
                    .unwrap_or(0);
                if held >= par.hold_ms && t.updates >= par.hold_updates {
                    State::Candidate
                } else {
                    State::Holding
                }
            }
        };
        view
    }

    /// Ставит паузу направления после сделки или ошибки (signal.route_cooldown_s).
    /// Принимает: направление, длительность паузы в секундах, время. Ошибок нет.
    pub fn cooldown(&mut self, dir: &Direction, secs: u64, now: DateTime<Utc>) {
        let t = self.map.entry(dir.clone()).or_default();
        t.cooldown_until = Some(now + Duration::seconds(secs as i64));
        t.above_since = None;
        t.updates = 0;
    }
}
