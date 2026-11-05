//! Файл: core/risk.rs
//! Что делает: стоп-условия и управление (шаг 6 ТЗ, экран «Управление»): режим системы (старт, стоп,
//! пауза), пауза площадки после N ошибок подряд, карантин монеты, остановка по убытку за день,
//! остановка новых сделок при расхождении сверки. Торговля не включается, пока не заданы все настройки
//! групп money.* и risk.* (решение 15); risk.max_errors_in_row — и у площадки продажи B (решение 118, Я-10).
//! Карантин — по адресу контракта (quarantine.rs, решение 118).
//! Лимит убытка за день — по площадке (daily_venue_limit.rs, решение 173: пауза только площадки) и общий «Стоп»
//! (daily_limit.rs, risk.max_daily_loss_total_usd; решения 53, 152, 153). Необязательные настройки (optional) не
//! требуются для включения торговли.
//! Кто вызывает: executor.rs (перед сделкой и при ошибках), daily_limit.rs (стоп по лимиту дня), transfers.rs
//! (карантин), api (команды управления через методы Core).
//! От чего зависит: engine.rs, settings (catalog, read), view.rs.
//! Настройки: risk.max_errors_in_row (по площадке), risk.max_daily_loss_usd (по площадке; у каждой площадки учёта
//! сделки), risk.max_daily_loss_total_usd (необязательна), все обязательные money.* и risk.*.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rust_decimal::Decimal;

use serde::{Deserialize, Serialize};

use crate::core::types::{Direction, VenueId};
use crate::settings::read::At;

pub use super::day_view::DayView;
use super::engine::{Core, lk};
use super::view::CoreEvent;

/// Режим системы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Новые сделки не открываются; после запуска система в этом режиме до команды «Старт».
    Stopped,
    Running,
    /// Новые сделки не открываются, идущие доводятся до конца.
    Paused,
}

/// Состояние риска — для экрана «Управление».
#[derive(Debug, Clone, Serialize)]
pub struct Risk {
    pub mode: Mode,
    /// Почему система не в работе (кто или что остановил).
    pub reason: Option<String>,
    pub venue_paused: HashMap<VenueId, String>,
    pub errors_in_row: HashMap<VenueId, u64>,
    /// Монеты в карантине — все их направления на паузе до ручного снятия. Ключ — адрес контракта (решение 118,
    /// quarantine.rs); в интерфейс — списком.
    #[serde(serialize_with = "super::quarantine::serialize")]
    pub quarantine: super::quarantine::Map,
    /// Итог дня (решения 53, 152, daily.rs): дата, всего, из них факт и оценка позиций; лимит дня (daily_limit.rs).
    pub day: DayView,
    /// Система остановлена именно по общему лимиту дня (daily_limit.rs); любая смена режима снимает отметку.
    pub daily_stop: bool,
    /// Решение 173: площадки на паузе по убытку дня (их причина — в venue_paused). Снимает «Снять паузу» или
    /// «Сбросить лимит дня» (по площадке или общий).
    #[serde(skip)]
    pub loss_paused: BTreeSet<String>,
    /// Решение 173: убыток площадки для лимита при прошлом пересчёте — пауза ставится при переходе через лимит или
    /// при росте убытка (daily_venue_limit.rs). Новый день — с нуля.
    #[serde(skip)]
    pub loss_seen: BTreeMap<String, Decimal>,
}

impl Default for Risk {
    fn default() -> Self {
        Risk {
            mode: Mode::Stopped,
            reason: Some("система запущена — ждёт команды «Старт»".into()),
            venue_paused: HashMap::new(),
            errors_in_row: HashMap::new(),
            quarantine: HashMap::new(),
            day: DayView::default(),
            daily_stop: false,
            loss_paused: BTreeSet::new(),
            loss_seen: BTreeMap::new(),
        }
    }
}

/// Публикует состояние управления в интерфейс. Ошибок нет.
pub fn publish(core: &Core) {
    let r = snapshot(core);
    if let Ok(v) = serde_json::to_value(r) {
        core.inner.live.send(CoreEvent::Control(v));
    }
}

/// Можно ли открыть новую сделку по направлению. Ошибки: текст причины.
pub fn can_open(core: &Core, d: &Direction) -> Result<(), String> {
    super::risk_ready::settings_ready(core, d)?;
    let r = lk(&core.inner.risk);
    if r.mode != Mode::Running {
        // Решение 164: режим по-русски («Стоп», «Пауза»), а не внутреннее имя.
        let why = r.reason.clone().unwrap_or_default();
        let why = if why.is_empty() {
            String::new()
        } else {
            format!(" — {why}")
        };
        return Err(format!(
            "торговля в режиме «{}»{why}",
            super::commands_control::label(r.mode)
        ));
    }
    for v in [&d.buy_on, &d.sell_on] {
        if let Some(why) = r.venue_paused.get(v) {
            return Err(format!("площадка {} на паузе: {why}", v.0));
        }
    }
    if let Some(q) = r.quarantine.get(&super::coin_key::CoinKey::of(&d.asset)) {
        return Err(format!("{} в карантине: {}", q.label, q.reason));
    }
    Ok(())
}

/// Команда управления (интерфейс): режим системы. Возвращает: текст для журнала.
pub fn set_mode(core: &Core, mode: Mode, reason: Option<String>) -> String {
    {
        let mut r = lk(&core.inner.risk);
        r.mode = mode;
        r.reason = reason.clone();
        r.daily_stop = false;
    }
    publish(core);
    format!(
        "Режим системы: {mode:?}{}",
        reason.map(|r| format!(" — {r}")).unwrap_or_default()
    )
}

/// Ошибка площадки: считает подряд; достигнут risk.max_errors_in_row — пауза площадки.
/// Возвращает: Some(текст) — площадка поставлена на паузу (для уведомления risk.stop).
pub fn on_error(core: &Core, venue: &VenueId) -> Option<String> {
    let max = core.inner.deps.settings.integer(
        "risk.max_errors_in_row",
        At {
            venue: Some(&venue.0),
            coin: None,
        },
    );
    let text = {
        let mut r = lk(&core.inner.risk);
        let n = r.errors_in_row.entry(venue.clone()).or_default();
        *n += 1;
        let n = *n;
        match max {
            Ok(m) if n >= m => {
                let t = format!("{n} ошибок подряд — предел {m}");
                r.venue_paused.insert(venue.clone(), t.clone());
                Some(t)
            }
            _ => None,
        }
    };
    publish(core);
    text
}

/// Успешная операция площадки — счётчик ошибок подряд обнуляется.
pub fn on_ok(core: &Core, venue: &VenueId) {
    lk(&core.inner.risk).errors_in_row.remove(venue);
}

/// Снять паузу площадки (интерфейс). Ошибок нет.
pub fn resume_venue(core: &Core, venue: &VenueId) {
    let mut r = lk(&core.inner.risk);
    r.venue_paused.remove(venue);
    r.errors_in_row.remove(venue);
    // Решение 173: пауза по убытку дня тоже снята; новая — только если убыток площадки вырастет.
    r.loss_paused.remove(&venue.0);
    drop(r);
    publish(core);
}

// Проверка дневного лимита убытка и «Сбросить лимит дня» — core/daily_limit.rs (решения 53, 152, 153).

/// Состояние для экрана «Управление»; лимиты дня — по текущим настройкам (daily_limit::view, daily_venue_limit::view).
pub fn snapshot(core: &Core) -> Risk {
    let mut r = lk(&core.inner.risk).clone();
    r.day.limit = super::daily_limit::view(core, &r.day);
    let date = r.day.date;
    for (v, x) in r.day.venues.iter_mut() {
        x.limit = super::daily_venue_limit::view(core, v, x, date);
        x.paused = r.loss_paused.contains(v);
    }
    r
}
