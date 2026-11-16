//! Файл: gateway/quote_purpose.rs
//! Что делает: решение 348 — замер «кто вызвал котировку». Назначение котировки — признак задачи tokio (task_local, как
//! gateway/urgency.rs): ядро помечает входы (сигнал, подбор объёма кандидата, предпроверка, «без потолка», теневой
//! расчёт, окно, карточка). Адаптер при входе в котировку берёт итоговую метку `who()`: финальная котировка
//! (quote_stage::is_final) и исполнение (patience::is_exec) распознаются своими признаками и сильнее назначения; без
//! назначения — класс срочности Check («проверки») или «не помечено» (поиск циклов DEX ↔ DEX и прочие вызовы адаптера,
//! которые ядро не помечает). Здесь же — дешёвые счётчики котировок по метке и виду (полная / быстрый путь
//! quote/fills.rs): атомики без замков, без записи на каждую котировку; сводку раз в log.quote_count_s пишет
//! core/quote_count_log.rs. Признак — только наблюдение: логика котировки от него не зависит.
//! Задача tokio признак не наследует: при запуске задачи его передают явно (как urgency); в поток счёта котировки DEX
//! (quote/cpu.rs) его переносит scope_opt.
//!
//! Кто вызывает: ядро — core/prices.rs (сигнал), core/candidates.rs (подбор), core/precheck.rs и core/refit.rs
//! (предпроверка), core/uncapped_queue.rs («без потолка»), core/busy_calc.rs (теневой), core/window_deal_run.rs (окно),
//! core/card_job.rs (карточка); адаптер DEX — venues/bsc_dex/quote/quote_timing_outer.rs (who, count),
//! venues/bsc_dex/quote/cpu.rs (current, scope_opt — перенос в поток счёта);
//! core/quote_count_log.rs (snapshot).
//! От чего зависит: tokio (task_local), gateway/quote_stage.rs, gateway/patience.rs, gateway/urgency.rs.
//! Настройки: нет (сводку включает log.quote_count_s — core/quote_count_log.rs).

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

use super::urgency::{self, Urgency};

/// Назначение котировки, которое ставит ядро на вход.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Пересчёт сигнала («Живой экран», core/prices.rs).
    Signal,
    /// Расчёт кандидата: подбор объёма (финальную котировку внутри распознаёт quote_stage).
    Size,
    /// Пересчёт перед покупкой (предпроверка, подбор под свободный остаток).
    Precheck,
    /// Фоновый расчёт «без потолка».
    Uncapped,
    /// Теневой расчёт занятой монеты (window.busy_calc).
    Shadow,
    /// Точка цены DEX в окне на объём сделки.
    Window,
    /// Карточка сигнала.
    Card,
}

/// Итоговая метка котировки (что пишется в запись и счётчики). Порядок — индекс счётчика.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Signal,
    Size,
    Final,
    Precheck,
    Uncapped,
    Shadow,
    Window,
    Card,
    Exec,
    Check,
    Unmarked,
}

/// Все метки по порядку индекса (для сводки).
pub const ALL: [Who; 11] = [
    Who::Signal,
    Who::Size,
    Who::Final,
    Who::Precheck,
    Who::Uncapped,
    Who::Shadow,
    Who::Window,
    Who::Card,
    Who::Exec,
    Who::Check,
    Who::Unmarked,
];

impl Who {
    /// Имя метки для журнала (поле who записи dex.quote_timing, ключи сводки dex.quote_count). Ошибок нет.
    pub fn name(self) -> &'static str {
        match self {
            Who::Signal => "signal",
            Who::Size => "size",
            Who::Final => "final",
            Who::Precheck => "precheck",
            Who::Uncapped => "uncapped",
            Who::Shadow => "shadow",
            Who::Window => "window",
            Who::Card => "card",
            Who::Exec => "exec",
            Who::Check => "check",
            Who::Unmarked => "unmarked",
        }
    }
}

tokio::task_local! {
    static PURPOSE: Purpose;
}

/// Выполняет работу с назначением `p`. Работа сразу кладётся в кучу (Box::pin) при вызове: будущее обёртки держит
/// только указатель, будущие вызывающих (расчёт кандидата, предпроверка, теневой, «без потолка») не растут на размер
/// работы — иначе переполнение стека (CI t160: core_busy_calc_267, core_dry_mode). Принимает: назначение, работу.
/// Возвращает: будущее с её результатом. Ошибок нет.
pub fn scope<F: Future>(p: Purpose, work: F) -> impl Future<Output = F::Output> {
    PURPOSE.scope(p, Box::pin(work))
}

/// Решение 348 (слияние с t157): перенос назначения в поток счёта котировки (venues/bsc_dex/quote/cpu.rs run — так же,
/// как класс срочности urgency::scope_opt): задача потока счёта признаков вызывающей не видит. None — без назначения.
/// Работа уже в куче у вызывающего (cpu.rs принимает Pin<Box>), здесь не боксится. Принимает: назначение, работу.
/// Возвращает: её итог. Ошибок нет.
pub async fn scope_opt<F: Future>(p: Option<Purpose>, work: F) -> F::Output {
    match p {
        Some(p) => PURPOSE.scope(p, work).await,
        None => work.await,
    }
}

/// Назначение текущей задачи, если его поставили. Ошибок нет.
pub fn current() -> Option<Purpose> {
    PURPOSE.try_with(|p| *p).ok()
}

/// Итоговая метка котировки на текущей задаче: финальная и исполнение сильнее назначения; без назначения — класс
/// Check или «не помечено». Ошибок нет.
pub fn who() -> Who {
    if super::quote_stage::is_final() {
        return Who::Final;
    }
    if super::patience::is_exec() {
        return Who::Exec;
    }
    match current() {
        Some(Purpose::Signal) => Who::Signal,
        Some(Purpose::Size) => Who::Size,
        Some(Purpose::Precheck) => Who::Precheck,
        Some(Purpose::Uncapped) => Who::Uncapped,
        Some(Purpose::Shadow) => Who::Shadow,
        Some(Purpose::Window) => Who::Window,
        Some(Purpose::Card) => Who::Card,
        None if urgency::is_set() == Some(Urgency::Check) => Who::Check,
        None => Who::Unmarked,
    }
}

/// Класс срочности текущей задачи именем для записи (не задан — «-»). Ошибок нет.
pub fn class_name() -> &'static str {
    match urgency::is_set() {
        Some(Urgency::Trade) => "trade",
        Some(Urgency::Check) => "check",
        Some(Urgency::Quote) => "quote",
        Some(Urgency::Background) => "background",
        None => "-",
    }
}

/// Счётчики: [метка × вид], вид 0 — полная котировка, 1 — быстрый путь (quote/fills.rs).
static COUNTS: [AtomicU64; ALL.len() * 2] = [const { AtomicU64::new(0) }; ALL.len() * 2];

/// Индекс метки в ALL. Ошибок нет.
fn index(w: Who) -> usize {
    ALL.iter().position(|x| *x == w).unwrap_or(ALL.len() - 1)
}

/// Отмечает одну котировку. Принимает: метку, быстрый путь ли. Ошибок нет.
pub fn count(w: Who, fast: bool) {
    COUNTS[index(w) * 2 + usize::from(fast)].fetch_add(1, Ordering::Relaxed);
}

/// Счётчики с запуска: по каждой метке (полных, быстрых). Ошибок нет.
pub fn snapshot() -> Vec<(Who, u64, u64)> {
    ALL.iter()
        .enumerate()
        .map(|(i, w)| {
            (
                *w,
                COUNTS[i * 2].load(Ordering::Relaxed),
                COUNTS[i * 2 + 1].load(Ordering::Relaxed),
            )
        })
        .collect()
}
