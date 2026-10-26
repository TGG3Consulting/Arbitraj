//! Файл: core/episode.rs
//! Что делает: эпизоды арбитражной ситуации (решение 74) — чистая логика без диска: эпизод направления открывается,
//! когда кандидат прошёл (Accept, в том числе сухой), и закрывается, когда направление перестало проходить (отказ
//! расчёта или сигнал вышел из «кандидата» — причина записывается). Пока эпизод открыт, каждый пересчёт — запись, но
//! не чаще episode.min_interval_ms (чаще — пропуск, чтобы не утонуть). Наибольшая прибыль и объём эпизода — здесь.
//! Запись на диск, журнал и Telegram — episode_run.rs.
//! Кто вызывает: core/episode_run.rs, core/candidates.rs (открыт ли, пора ли пересчитать), тесты (tests/core_episodes.rs).
//! От чего зависит: core/types (Direction), chrono, rust_decimal.
//! Настройки: нет (интервал передаёт вызывающий из episode.min_interval_ms).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::Direction;

/// Сводка эпизода — строка экрана «Эпизоды» (хранится в SQLite, storage/episodes_db.rs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeInfo {
    pub id: String,
    pub direction: Direction,
    /// Открыт в сухом режиме.
    pub dry: bool,
    pub started_at: DateTime<Utc>,
    /// None — эпизод ещё открыт.
    pub ended_at: Option<DateTime<Utc>>,
    /// Наибольшая прибыль на бюджете и наибольшая без бюджета (если считалась), USDT.
    pub max_profit: Decimal,
    pub max_profit_unbounded: Option<Decimal>,
    /// Сколько записей пересчёта в файле.
    pub records: u64,
    pub close_reason: Option<String>,
    /// Запись остановлена (лимит места episode.max_mb, ошибка диска) — почему.
    pub stopped: Option<String>,
}

/// Что сделать с прошедшим кандидатом.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Эпизод открыт сейчас.
    Opened(EpisodeInfo),
    /// Эпизод уже открыт — записать пересчёт.
    Record(EpisodeInfo),
    /// Эпизод открыт, но с прошлой записи прошло меньше интервала — пропуск.
    Skip,
}

/// Открытый эпизод.
#[derive(Debug, Clone)]
struct Open {
    info: EpisodeInfo,
    last: DateTime<Utc>,
}

/// Открытые эпизоды по направлениям.
#[derive(Debug, Default)]
pub struct Episodes {
    open: HashMap<Direction, Open>,
}

impl Episodes {
    /// Открыт ли эпизод направления. Ошибок нет.
    pub fn is_open(&self, d: &Direction) -> bool {
        self.open.contains_key(d)
    }

    /// Пора ли пересчитать открытый эпизод: прошло не меньше интервала с прошлой записи (закрытый — всегда пора).
    /// Принимает: направление, время, интервал мс. Ошибок нет.
    pub fn due(&self, d: &Direction, now: DateTime<Utc>, min_ms: u64) -> bool {
        self.open
            .get(d)
            .is_none_or(|o| (now - o.last).num_milliseconds() >= min_ms as i64)
    }

    /// Что делает: кандидат прошёл — открыть эпизод или записать пересчёт.
    /// Принимает: направление, id для нового эпизода, сухой ли режим, прибыль на бюджете, без бюджета (если есть),
    /// время, интервал записей мс. Возвращает: Step. Ошибок нет.
    #[allow(clippy::too_many_arguments)]
    pub fn accept(
        &mut self,
        d: &Direction,
        id: String,
        dry: bool,
        profit: Decimal,
        unbounded: Option<Decimal>,
        now: DateTime<Utc>,
        min_ms: u64,
    ) -> Step {
        if let Some(o) = self.open.get_mut(d) {
            if (now - o.last).num_milliseconds() < min_ms as i64 {
                return Step::Skip;
            }
            o.last = now;
            o.info.records += 1;
            o.info.max_profit = o.info.max_profit.max(profit);
            o.info.max_profit_unbounded = max_opt(o.info.max_profit_unbounded, unbounded);
            return Step::Record(o.info.clone());
        }
        let info = EpisodeInfo {
            id,
            direction: d.clone(),
            dry,
            started_at: now,
            ended_at: None,
            max_profit: profit,
            max_profit_unbounded: unbounded,
            records: 1,
            close_reason: None,
            stopped: None,
        };
        self.open.insert(
            d.clone(),
            Open {
                info: info.clone(),
                last: now,
            },
        );
        Step::Opened(info)
    }

    /// Что делает: направление перестало проходить — закрыть эпизод.
    /// Принимает: направление, причину, время. Возвращает: закрытый эпизод; None — открытого не было. Ошибок нет.
    pub fn close(
        &mut self,
        d: &Direction,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Option<EpisodeInfo> {
        let mut info = self.open.remove(d)?.info;
        info.ended_at = Some(now);
        info.close_reason = Some(reason.to_string());
        Some(info)
    }

    /// Отметка «запись остановлена» у открытого эпизода. Принимает: направление, причину. Ошибок нет.
    pub fn stop(&mut self, d: &Direction, why: &str) {
        if let Some(o) = self.open.get_mut(d) {
            o.info.stopped = Some(why.to_string());
        }
    }

    /// Наибольшая прибыль без бюджета из карточки (строится после записи). Принимает: направление, прибыль.
    /// Возвращает: сводку открытого эпизода; None — эпизод закрыт. Ошибок нет.
    pub fn note_unbounded(&mut self, d: &Direction, v: Option<Decimal>) -> Option<EpisodeInfo> {
        let o = self.open.get_mut(d)?;
        o.info.max_profit_unbounded = max_opt(o.info.max_profit_unbounded, v);
        Some(o.info.clone())
    }

    /// Открытые эпизоды (для экрана). Ошибок нет.
    pub fn list(&self) -> Vec<EpisodeInfo> {
        self.open.values().map(|o| o.info.clone()).collect()
    }
}

/// Больший из двух необязательных. Ошибок нет.
fn max_opt(a: Option<Decimal>, b: Option<Decimal>) -> Option<Decimal> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}
