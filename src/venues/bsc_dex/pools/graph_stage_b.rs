//! Файл: venues/bsc_dex/pools/graph_stage_b.rs
//! Что делает: решение 308 (задача t137) — состояние ступени Б фонового графа (методы Bg; вынесено из graph_snap.rs):
//! - признак «догоняющая сборка» (карты нет и идёт полная сборка, или идёт доводка): действующий снимок отдаётся и
//!   отставшим не считается (graph_read.rs). Владелец пишет признак только под проверкой поколения в том же замке, где
//!   сторож меняет поколение (set_catching_for; третье ревью п. 4) — снятый владелец не затрёт признак сторожа;
//! - длительность ступени Б — наибольшее из двух отсчётов ВЛАДЕЛЬЦА: догоняющая сборка/доводка и «действующий снимок
//!   собран на прежних настройках графа» (четвёртое ревью п. 1: не котировки — иначе отсчёт застревал, когда котировки
//!   перестали читать снимок). Оба сбрасываются публикацией снимка на текущих настройках, снятием снимка и выключением
//!   режима (end_stage_b). Отсчёт после перезапуска начинается заново, когда новый владелец начал работу (после паузы
//!   dex.graph_retry_pause_s — пауза в отсчёт не входит; третье ревью п. 1а) — поэтому полная длительность ступени с
//!   перезапуском по пределу недосчитывается на время до перезапуска (четвёртое ревью б);
//! - предел ступени Б dex.graph_stage_b_max_ms (сторож, graph_guard.rs): первое превышение в ступени — один перезапуск
//!   (может вылечить зависшую сборку); повторное превышение в той же ступени владельца больше НЕ снимает (перезапуск не
//!   ускоряет нормально идущую сборку — иначе петля при любом пределе меньше времени сборки), а снимает льготу: снимок
//!   снова проверяется предохранителем dex.graph_max_lag_ms (не задан — отдаётся, цена свежая), уведомление и экран.
//!   Оба признака сбрасываются, когда ступень Б кончилась (третье ревью п. 1в);
//! - срок повтора неудавшейся полной сборки (dex.graph_retry_pause_s) — переживает перезапуск владельца.
//!
//! Кто вызывает: graph_step.rs, graph_finish.rs (set_catching_for), graph_guard.rs, graph_read.rs, graph_stats.rs.
//! От чего зависит: graph_snap.rs (Bg), graph_alarm.rs (отсчёт котировок).
//! Настройки: нет (значения пределов читают вызывающие).

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::graph_snap::Bg;

impl Bg {
    /// Сторож: признак догоняющей сборки без проверки поколения (после сдвига поколения). Ошибок нет.
    pub fn set_catching_up(&self, on: bool) {
        let mut c = self
            .catching_since
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        set(&mut c, on);
    }

    /// Владелец: признак догоняющей сборки, только если `generation` — текущее поколение (проверка и запись — под одним
    /// замком со сдвигом поколения). Возвращает: записан ли. Ошибок нет.
    pub fn set_catching_for(&self, generation: u64, on: bool) -> bool {
        let mut c = self
            .catching_since
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !self.is_current(generation) {
            return false;
        }
        set(&mut c, on);
        true
    }

    /// Идёт ли догоняющая полная сборка/доводка. Ошибок нет.
    pub fn catching_up(&self) -> bool {
        self.catching_age().is_some()
    }

    /// Сколько длится догоняющая сборка/доводка (None — не идёт). Ошибок нет.
    pub fn catching_age(&self) -> Option<Duration> {
        self.catching_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|t| t.elapsed())
    }

    /// Длительность ступени Б — наибольшее из двух отсчётов владельца (см. шапку); None — ступени нет. Ошибок нет.
    pub fn stage_b_age(&self) -> Option<Duration> {
        let stale = self
            .stale_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|t| t.elapsed());
        self.catching_age().max(stale)
    }

    /// Владелец: действующий снимок собран на прежних настройках графа (`on`) — второй отсчёт ступени Б; только для
    /// текущего поколения (как set_catching_for). Ошибок нет.
    pub fn set_stale_for(&self, generation: u64, on: bool) {
        let mut c = self.stale_since.lock().unwrap_or_else(|p| p.into_inner());
        if self.is_current(generation) {
            set(&mut c, on);
        }
    }

    /// Ступень Б кончилась (снимок на текущих настройках опубликован, снимок снят, режим выключен): оба отсчёта и
    /// признаки перезапуска/просрочки сброшены (четвёртое ревью п. 1). Ошибок нет.
    pub fn end_stage_b(&self) {
        *self
            .catching_since
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        *self.stale_since.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.clear_stage_b_marks();
    }

    /// То же, что end_stage_b, только для текущего поколения: проверка и сброс — под замком признака догоняющей сборки,
    /// который держит смена поколения (пятое ревью б). Ошибок нет.
    pub fn end_stage_b_for(&self, generation: u64) {
        let mut c = self
            .catching_since
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !self.is_current(generation) {
            return;
        }
        *c = None;
        *self.stale_since.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.clear_stage_b_marks();
    }

    /// Четвёртое ревью (а): переход режима «выкл → вкл» (true ровно один раз на переход). Ошибок нет.
    pub fn mode_switched_on(&self, on: bool) -> bool {
        !self.mode_on.swap(on, Ordering::AcqRel) && on
    }

    /// Сторож перед запуском нового владельца (после паузы): отсчёт ступени Б — заново, с этого момента. Ошибок нет.
    pub fn restart_stage_b_clock(&self) {
        for m in [&self.catching_since, &self.stale_since] {
            let mut c = m.lock().unwrap_or_else(|p| p.into_inner());
            if c.is_some() {
                *c = Some(Instant::now());
            }
        }
        self.alarm.restart_stage_b();
    }

    /// Был ли уже перезапуск по пределу в этой ступени Б. Ошибок нет.
    pub fn stage_b_restarted(&self) -> bool {
        self.stage_b_restarted.load(Ordering::Acquire)
    }

    /// Отметить перезапуск по пределу ступени Б. Ошибок нет.
    pub fn mark_stage_b_restarted(&self) {
        self.stage_b_restarted.store(true, Ordering::Release);
    }

    /// Снята ли льгота ступени Б (просрочена после перезапуска). Ошибок нет.
    pub fn stage_b_overdue(&self) -> bool {
        self.stage_b_overdue.load(Ordering::Acquire)
    }

    /// Снять льготу ступени Б. Возвращает: true — снята сейчас (впервые в этой ступени). Ошибок нет.
    pub fn mark_stage_b_overdue(&self) -> bool {
        !self.stage_b_overdue.swap(true, Ordering::AcqRel)
    }

    /// Ступень Б кончилась — признаки перезапуска и просрочки сброшены. Ошибок нет.
    pub fn clear_stage_b_marks(&self) {
        self.stage_b_restarted.store(false, Ordering::Release);
        self.stage_b_overdue.store(false, Ordering::Release);
    }

    /// Назначить (или снять) срок повтора неудавшейся полной сборки. Ошибок нет.
    pub fn set_retry_at(&self, at: Option<tokio::time::Instant>) {
        *self.retry_at.lock().unwrap_or_else(|p| p.into_inner()) = at;
    }

    /// Срок повтора; прошедший — снимается (None). Ошибок нет.
    pub fn retry_at(&self) -> Option<tokio::time::Instant> {
        let mut r = self.retry_at.lock().unwrap_or_else(|p| p.into_inner());
        if r.is_some_and(|t| tokio::time::Instant::now() >= t) {
            *r = None;
        }
        *r
    }
}

/// Взвести/снять отметку времени признака: взводится с текущего момента, повторный взвод время не меняет. Ошибок нет.
fn set(c: &mut Option<Instant>, on: bool) {
    match (on, c.is_some()) {
        (true, false) => *c = Some(Instant::now()),
        (false, true) => *c = None,
        _ => {}
    }
}
