//! Файл: core/risk.rs
//! Что делает: стоп-условия и управление (шаг 6 ТЗ, экран «Управление»): режим системы (старт, стоп,
//! пауза), пауза площадки после N ошибок подряд, карантин монеты, остановка по убытку за день,
//! остановка новых сделок при расхождении сверки. Торговля не включается, пока не заданы все настройки
//! групп money.* и risk.* (решение 15).
//! Кто вызывает: executor.rs (перед сделкой и при ошибках), ledger.rs (итог дня, сверка), transfers.rs
//! (карантин), api (команды управления через методы Core).
//! От чего зависит: engine.rs, settings (catalog, read), view.rs.
//! Настройки: risk.max_errors_in_row (по площадке), risk.max_daily_loss_usd, все money.* и risk.*.

use std::collections::HashMap;

use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, Direction, VenueId};
use crate::settings::catalog;
use crate::settings::read::At;

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
    /// Монеты в карантине — все их направления на паузе до ручного снятия.
    pub quarantine: HashMap<String, String>,
    pub day: NaiveDate,
    pub day_result_usd: Decimal,
}

impl Default for Risk {
    fn default() -> Self {
        Risk {
            mode: Mode::Stopped,
            reason: Some("система запущена — ждёт команды «Старт»".into()),
            venue_paused: HashMap::new(),
            errors_in_row: HashMap::new(),
            quarantine: HashMap::new(),
            day: Utc::now().date_naive(),
            day_result_usd: Decimal::ZERO,
        }
    }
}

/// Публикует состояние управления в интерфейс. Ошибок нет.
pub fn publish(core: &Core) {
    let r = lk(&core.inner.risk).clone();
    if let Ok(v) = serde_json::to_value(r) {
        core.inner.live.send(CoreEvent::Control(v));
    }
}

/// Можно ли открыть новую сделку по направлению. Ошибки: текст причины.
pub fn can_open(core: &Core, d: &Direction) -> Result<(), String> {
    let s = &core.inner.deps.settings;
    for def in
        catalog::all().filter(|d| d.name.starts_with("money.") || d.name.starts_with("risk."))
    {
        if s.resolve(def.name, Some(&d.buy_on.0), None).is_none() {
            return Err(format!(
                "торговля не включается: не задана настройка {}",
                def.name
            ));
        }
    }
    let r = lk(&core.inner.risk);
    if r.mode != Mode::Running {
        return Err(format!(
            "система не в работе: {:?} ({})",
            r.mode,
            r.reason.clone().unwrap_or_default()
        ));
    }
    for v in [&d.buy_on, &d.sell_on] {
        if let Some(why) = r.venue_paused.get(v) {
            return Err(format!("площадка {} на паузе: {why}", v.0));
        }
    }
    if let Some(why) = r.quarantine.get(&d.asset.ticker) {
        return Err(format!("{} в карантине: {why}", d.asset.ticker));
    }
    Ok(())
}

/// Команда управления (интерфейс): режим системы. Возвращает: текст для журнала.
pub fn set_mode(core: &Core, mode: Mode, reason: Option<String>) -> String {
    {
        let mut r = lk(&core.inner.risk);
        r.mode = mode;
        r.reason = reason.clone();
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
                let t = format!("{n} ошибок подряд (risk.max_errors_in_row = {m})");
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
    drop(r);
    publish(core);
}

/// Карантин монеты (Some — поставить с причиной, None — снять). Ошибок нет.
pub fn quarantine(core: &Core, asset: &Asset, reason: Option<String>) {
    {
        let mut r = lk(&core.inner.risk);
        match reason {
            Some(t) => r.quarantine.insert(asset.ticker.clone(), t),
            None => r.quarantine.remove(&asset.ticker),
        };
    }
    publish(core);
}

/// Итог закрытой сделки — в убыток или прибыль дня. Убыток дня достиг risk.max_daily_loss_usd —
/// система останавливается до ручного решения. Возвращает: Some(текст) — остановлена.
pub fn add_result(core: &Core, result: Decimal) -> Option<String> {
    let max = core
        .inner
        .deps
        .settings
        .decimal("risk.max_daily_loss_usd", At::default());
    let stop = {
        let mut r = lk(&core.inner.risk);
        let today = Utc::now().date_naive();
        if r.day != today {
            r.day = today;
            r.day_result_usd = Decimal::ZERO;
        }
        r.day_result_usd += result;
        match max {
            Ok(m) if -r.day_result_usd >= m => {
                let t = format!(
                    "убыток за день {} USDT достиг лимита {m}",
                    -r.day_result_usd
                );
                r.mode = Mode::Stopped;
                r.reason = Some(t.clone());
                Some(t)
            }
            _ => None,
        }
    };
    publish(core);
    stop
}

/// Состояние для экрана «Управление».
pub fn snapshot(core: &Core) -> Risk {
    lk(&core.inner.risk).clone()
}
