//! Файл: venues/bsc_dex/pools/graph_stats.rs
//! Что делает: решение 308 (задача t137) — счётчики фонового владельца графа для экрана «Площадки» (core/types/dex_graph.rs
//! DexGraphState): срабатывания предохранителя и ступени «снимок на прежних настройках» (поминутно, показ за час и сутки),
//! наибольшее отставание при публикации за час, расхождения самопроверки (последнее — текстом), перезапуски, последний
//! накопительный круг и последняя полная пересборка; view — сборка состояния для экрана.
//! Окна «за час» и «за сутки» — единицы показа, которые просил Тигран (поручение 16.11, п. 3), а не пороги поведения:
//! поэтому 60 и 1440 минут здесь — перевод единиц времени, не настройка.
//! Кто вызывает: graph_read.rs, graph_alarm.rs, graph_owner.rs, graph_full.rs, graph_guard.rs (запись); venue.rs (view).
//! От чего зависит: graph_snap.rs (Bg), graph_read.rs (режим), registry.rs, core/types (DexGraph*), chrono.
//! Настройки: через graph_read.rs — dex.graph_background.

use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::core::types::{DexGraphFull, DexGraphRound, DexGraphState};

use super::super::ctx::Ctx;
use super::graph_read;
use super::registry::Registry;

/// Минут в окне «за час». Не настройка поведения: окна «за час / за сутки» задал Тигран как единицы показа на экране
/// (поручение 16.11, п. 3), на торговлю и на работу графа они не влияют — это перевод часа в минуты (ревью №2 J).
const HOUR_MIN: i64 = 60;
/// Минут в окне «за сутки» (единица показа, как HOUR_MIN; старше суток значения выбрасываются — память ограничена окном).
const DAY_MIN: i64 = 24 * 60;

/// Счётчики. Один на реестр (в Bg).
#[derive(Default)]
pub struct GraphStats {
    m: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    fuse: Buckets,
    episodes: Buckets,
    reader: Buckets,
    reader_b: Buckets,
    stage_b: Buckets,
    stale: Buckets,
    lag: Buckets,
    mismatches: u64,
    last_mismatch: Option<(DateTime<Utc>, String)>,
    restarts: u64,
    last_restart: Option<String>,
    round: Option<DexGraphRound>,
    full: Option<DexGraphFull>,
}

/// Поминутные значения: (номер минуты UTC, сумма или наибольшее).
#[derive(Default)]
struct Buckets {
    q: VecDeque<(i64, u64)>,
}

/// Номер текущей минуты UTC. Ошибок нет.
fn minute() -> i64 {
    Utc::now().timestamp().div_euclid(60)
}

impl Buckets {
    /// Прибавить `n` к текущей минуте (sum) или взять наибольшее (max). Старше суток — выбросить. Ошибок нет.
    fn put(&mut self, n: u64, max: bool) {
        let now = minute();
        while self.q.front().is_some_and(|(m, _)| now - m >= DAY_MIN) {
            self.q.pop_front();
        }
        match self.q.back_mut() {
            Some((m, v)) if *m == now => *v = if max { (*v).max(n) } else { *v + n },
            _ => self.q.push_back((now, n)),
        }
    }

    /// Сумма за последние `window` минут. Ошибок нет.
    fn sum(&self, window: i64) -> u64 {
        let now = minute();
        self.q
            .iter()
            .filter(|(m, _)| now - m < window)
            .map(|(_, v)| v)
            .sum()
    }

    /// Наибольшее за последние `window` минут. Ошибок нет.
    fn top(&self, window: i64) -> Option<u64> {
        let now = minute();
        self.q
            .iter()
            .filter(|(m, _)| now - m < window)
            .map(|(_, v)| *v)
            .max()
    }
}

impl GraphStats {
    /// Предохранитель сработал (снимок отстал сверх dex.graph_max_lag_ms). Ошибок нет.
    pub fn fuse(&self) {
        self.lock().fuse.put(1, false);
    }

    /// Начался эпизод предохранителя (ревью №2 I). Ошибок нет.
    pub fn fuse_episode(&self) {
        self.lock().episodes.put(1, false);
    }

    /// Отставание, которое увидел расчёт при чтении снимка, мс (ревью №2 C). Ошибок нет.
    pub fn reader_lag(&self, ms: u64) {
        self.lock().reader.put(ms, true);
    }

    /// Отставание, которое увидел расчёт в ступени Б (предохранитель там не действует), мс — отдельно (повторное ревью
    /// п. 6). Ошибок нет.
    pub fn reader_lag_b(&self, ms: u64) {
        self.lock().reader_b.put(ms, true);
    }

    /// Ступень Б закончилась публикацией снимка, собранного с нуля: её полная длительность, мс (третье ревью п. 2).
    /// Ошибок нет.
    pub fn stage_b_done(&self, ms: f64) {
        self.lock().stage_b.put(ms as u64, true);
    }

    /// Отдан снимок на прежних настройках графа (ступень Б). Ошибок нет.
    pub fn stale_settings(&self) {
        self.lock().stale.put(1, false);
    }

    /// Расхождение самопроверки (текст — перечень отличий). Ошибок нет.
    pub fn mismatch(&self, text: String) {
        let mut m = self.lock();
        m.mismatches += 1;
        m.last_mismatch = Some((Utc::now(), text));
    }

    /// Владелец перезапущен сторожем (причина). Ошибок нет.
    pub fn restart(&self, why: &str) {
        let mut m = self.lock();
        m.restarts += 1;
        m.last_restart = Some(format!(
            "{} — {why}",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }

    /// Накопительный круг опубликован. Ошибок нет.
    pub fn round(&self, r: DexGraphRound) {
        let mut m = self.lock();
        m.lag.put(r.lag_ms, true);
        m.round = Some(r);
    }

    /// Полная пересборка закончилась (удачно или нет). Ошибок нет.
    pub fn full(&self, f: DexGraphFull) {
        self.lock().full = Some(f);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.m.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Состояние для экрана. Принимает: окружение, реестр. Возвращает: DexGraphState. Ошибок нет.
pub fn view(ctx: &Ctx, reg: &Registry) -> DexGraphState {
    let bg = reg.graph_cache().bg();
    let snap = bg.load();
    let m = bg.stats.lock();
    let ms = |d: std::time::Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    DexGraphState {
        enabled: graph_read::background(ctx),
        running: bg.started(),
        ready: snap.is_some(),
        why_none: if snap.is_none() { bg.why_none() } else { None },
        snapshot_version: snap.as_ref().map(|s| s.version),
        registry_version: reg.version(),
        seq: snap.as_ref().map(|s| s.seq),
        snapshot_full: snap.as_ref().map(|s| s.full),
        age_ms: snap.as_ref().map(|s| ms(s.at.elapsed())),
        lag_ms: bg.changes.lag().map(ms),
        reader_lag_max_hour_ms: m.reader.top(HOUR_MIN),
        stage_b_lag_max_hour_ms: m.reader_b.top(HOUR_MIN),
        stage_b_now_ms: bg.stage_b_age().map(ms),
        stage_b_full_max_hour_ms: m.stage_b.top(HOUR_MIN),
        stage_b_overdue: bg.stage_b_overdue(),
        publish_lag_max_hour_ms: m.lag.top(HOUR_MIN),
        pending: bg.changes.pending(),
        edges: snap.as_ref().map(|s| s.graph.len()),
        last_round: m.round.clone(),
        last_full: m.full.clone(),
        mismatches: m.mismatches,
        last_mismatch_at: m.last_mismatch.as_ref().map(|x| x.0),
        last_mismatch: m.last_mismatch.as_ref().map(|x| x.1.clone()),
        fuse_hour: m.fuse.sum(HOUR_MIN),
        fuse_day: m.fuse.sum(DAY_MIN),
        fuse_episodes_hour: m.episodes.sum(HOUR_MIN),
        fuse_episodes_day: m.episodes.sum(DAY_MIN),
        stale_settings_hour: m.stale.sum(HOUR_MIN),
        stale_settings_day: m.stale.sum(DAY_MIN),
        restarts: m.restarts,
        last_restart: m.last_restart.clone(),
    }
}
