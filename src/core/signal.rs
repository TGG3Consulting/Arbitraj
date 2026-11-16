//! Файл: core/signal.rs
//! Что делает: грубый спред по каждому направлению монеты и фильтр шума (шаг 3 ТЗ).
//! Грубый спред A→B = (цена продажи на B − цена покупки на A) / цена покупки на A, в процентах.
//! Порог перехода к полному расчёту — сумма процентных комиссий обеих сторон, которых нет в цене
//! (ТЗ, шаг 3: «ниже этого заработать нельзя ни на каком объёме»). Сигнал отбрасывается, если спред
//! мигает (держится меньше signal.min_hold_ms или меньше signal.min_hold_updates обновлений подряд),
//! аномально велик (выше signal.anomaly_spread_pct — нужна перепроверка статусов ввода/вывода)
//! или по направлению идёт пауза после сделки или ошибки (signal.route_cooldown_s).
//! Решение 317: при signal.dex_event_skip_hold = да кандидат от события пула DEX выдаётся без удержания, если цена
//! стороны биржи не менялась не меньше signal.min_hold_ms (signal_fast.rs); не задана — побайтно прежний фильтр.
//! Решение 318: при signal.hold_timer = да у «держится» есть срок дозревания ripen_at = max(above_since +
//! signal.min_hold_ms, сейчас) — по нему таймер (signal_ripen.rs) сам ставит пересчёт; не задана — срока нет.
//! Кто вызывает: core/evaluate.rs на каждый пересчёт монеты (после prices.rs).
//! От чего зависит: core/types, core/signal_fast.rs, settings.
//! Настройки: signal.min_hold_ms, signal.min_hold_updates, signal.anomaly_spread_pct, signal.route_cooldown_s,
//! signal.dex_event_skip_hold (решение 317), signal.hold_timer (решение 318).

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;

use super::signal_fast::{Ctx, Sides};
use crate::core::types::Direction;
use crate::settings::read::At;
use crate::settings::store::Store;

/// Пороги фильтра шума — из настроек, читаются на каждый тик (меняются на лету).
/// Default — только для литералов тестов (`..Default::default()`: необязательные правила выключены, как «не задана»);
/// движок берёт пороги только из настроек (Params::read).
#[derive(Default)]
pub struct Params {
    pub hold_ms: u64,
    pub hold_updates: u64,
    pub anomaly_pct: Decimal,
    /// Решение 317: signal.dex_event_skip_hold (необязательна; не задана — нет).
    pub skip_on_pool: bool,
    /// Решение 318: signal.hold_timer (необязательна; не задана — нет).
    pub ripen: bool,
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
            skip_on_pool: s.flag("signal.dex_event_skip_hold", at).unwrap_or(false),
            ripen: s.flag("signal.hold_timer", at).unwrap_or(false),
        })
    }
}

pub use super::signal_types::{Prices, SpreadView, State};

/// Память фильтра по направлению.
#[derive(Default)]
struct Track {
    above_since: Option<DateTime<Utc>>,
    updates: u64,
    cooldown_until: Option<DateTime<Utc>>,
    /// Решение 317: цены сторон биржа ↔ DEX и с каких пор стоит цена биржи (signal_fast.rs).
    sides: Sides,
}

/// Фильтр шума по всем направлениям.
#[derive(Default)]
pub struct Signals {
    map: HashMap<Direction, Track>,
}

impl Signals {
    /// Обновляет направление новым тиком без обстоятельств пересчёта (как до решения 317).
    /// Принимает: направление, цены (Err — причина, почему цены нет), пороги, время.
    /// Возвращает: строку «Живого экрана». Ошибок нет.
    pub fn update(
        &mut self,
        dir: &Direction,
        prices: Result<Prices, String>,
        params: &Result<Params, String>,
        now: DateTime<Utc>,
    ) -> SpreadView {
        self.update_with(dir, prices, params, now, &Ctx::default())
    }

    /// Обновляет направление пересчётом. Принимает: как update и обстоятельства пересчёта (решение 317: сторона DEX,
    /// что запустило). Возвращает: строку «Живого экрана». Ошибок нет.
    pub fn update_with(
        &mut self,
        dir: &Direction,
        prices: Result<Prices, String>,
        params: &Result<Params, String>,
        now: DateTime<Utc>,
        ctx: &Ctx,
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
            trigger: ctx.trigger,
            skipped_hold: false,
            cex_still_ms: None,
            ripen_at: None,
        };
        let p = match prices {
            Ok(p) => p,
            Err(e) => {
                // Нет цены — счётчики и память сторон (решение 317) сбрасываются: «стоит» только то, что видели.
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
        // Решение 317: стороны наблюдаются на каждом пересчёте, и ниже порога тоже — цена биржи «стоит» с тех пор.
        let seen = t.sides.observe(ctx, p.buy, p.sell, now);
        view.cex_still_ms = seen.map(|s| s.cex_still_ms);
        if spread <= p.fees_pct {
            t.above_since = None;
            t.updates = 0;
            return view;
        }
        t.above_since.get_or_insert(now);
        t.updates += 1;
        view.above_since = t.above_since;
        view.updates = t.updates;
        let state = match (params, t.cooldown_until) {
            (Err(e), _) => State::NotConfigured(e.clone()),
            (_, Some(until)) if until > now => State::Cooldown { until },
            (Ok(par), _) if spread > par.anomaly_pct => State::Anomaly,
            (Ok(par), _) => {
                let held = t
                    .above_since
                    .map(|s| (now - s).num_milliseconds().max(0) as u64)
                    .unwrap_or(0);
                let skip = super::signal_fast::skip_hold(
                    par.skip_on_pool,
                    par.hold_ms,
                    ctx,
                    seen.as_ref(),
                );
                if held >= par.hold_ms && t.updates >= par.hold_updates {
                    State::Candidate
                } else if skip {
                    view.skipped_hold = true;
                    State::Candidate
                } else {
                    // Решение 318: срок дозревания — когда пройдёт удержание; прошло, а обновлений мало — сейчас
                    // (следующий пересчёт — не раньше signal.eval_min_interval_ms, его держит tick.rs).
                    if par.ripen {
                        view.ripen_at = t
                            .above_since
                            .map(|s| (s + Duration::milliseconds(par.hold_ms as i64)).max(now));
                    }
                    State::Holding
                }
            }
        };
        view.state = state;
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
