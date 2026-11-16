//! Файл: gateway/cpu_budget.rs
//! Что делает: решение 336 (задача t157) — общий предел потоков тяжёлого счёта процесса с приоритетом: путь сделки выше
//! расчёта кандидатов, кандидаты выше фона. Тяжёлый счёт — котировка DEX вне рабочих потоков tokio (адаптер DEX,
//! quote/cpu.rs) и её вспомогательные потоки (отбор путей, сплит, сборка графа, поиск циклов — quote/par.rs). Каждый
//! такой поток держит одно место. Мест — cpu.threads_max; из них cpu.reserve_deal берёт только путь сделки, а
//! cpu.reserve_calc — только путь сделки и кандидаты (фон их не берёт). Так под приём WebSocket, таймеры, узел и
//! исполнение остаются ядра (рабочие потоки tokio вне предела), а фон не вытесняет кандидатов, кандидаты — сделку.
//! Ярусы (Tier) по признакам задачи шлюза: финальная котировка (quote_stage) или котировка исполнения (patience) —
//! путь сделки; класс срочности Trade (urgency) — кандидат; остальное (живой экран, проверки, фон) — фон.
//! Ярус может задать вызывающий на задачу (scope): путь сделки — core/executor.rs (вся задача сделки, в т.ч. предпроверка),
//! план цикла DEX ↔ DEX — core/cycle_plan.rs (кандидат). Освобождение будит одного ждущего самого важного яруса.
//! Ожидание места — внутри срока расчёта (profit.calc_timeout_ms): брошенный расчёт бросает и ожидание (место не
//! занимает). Смена настроек на лету: занятые места доживают, новые — по новым числам.
//! Не задана cpu.threads_max или 0 — общего предела нет (как до 336).
//! Кто вызывает: venues/bsc_dex/quote/cpu.rs (acquire), venues/bsc_dex/quote/par.rs (try_acquire), core/executor.rs и
//! core/cycle_plan.rs (scope).
//! От чего зависит: settings (чтение), gateway/urgency.rs (класс), tokio (Notify).
//! Настройки: cpu.threads_max, cpu.reserve_deal, cpu.reserve_calc (вся система; необязательные).

use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;

use crate::settings::read::At;
use crate::settings::store::Store;

use super::urgency::Urgency;

/// Ярус счёта. Меньше — важнее.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Путь сделки: финальная котировка, котировка исполнения.
    Deal,
    /// Расчёт кандидата (класс Trade без признака финальной).
    Calc,
    /// Всё остальное: живой экран, проверки, фон.
    Background,
}

/// Пределы из настроек.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// cpu.threads_max — мест всего.
    pub total: usize,
    /// cpu.reserve_deal — мест только для пути сделки.
    pub reserve_deal: usize,
    /// cpu.reserve_calc — мест только для пути сделки и кандидатов.
    pub reserve_calc: usize,
}

impl Limits {
    /// Сколько мест всего может быть занято, чтобы ярус `t` взял ещё одно. Ошибок нет.
    pub fn cap(&self, t: Tier) -> usize {
        match t {
            Tier::Deal => self.total,
            Tier::Calc => self.total.saturating_sub(self.reserve_deal),
            Tier::Background => self
                .total
                .saturating_sub(self.reserve_deal)
                .saturating_sub(self.reserve_calc),
        }
    }
}

/// Занятые места, сколько ждёт каждый ярус и сигнал «место освободилось» на каждый ярус. Один на процесс.
struct Budget {
    used: Mutex<usize>,
    /// Ждущие по ярусам: Deal, Calc, Background.
    waiting: [AtomicUsize; 3],
    /// Сигнал яруса: освобождение будит ОДНОГО ждущего самого важного ждущего яруса (ревью t157 Н2: не всех).
    freed: [Notify; 3],
}

static BUDGET: Budget = Budget {
    used: Mutex::new(0),
    waiting: [
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    ],
    freed: [
        Notify::const_new(),
        Notify::const_new(),
        Notify::const_new(),
    ],
};

/// Номер яруса в массивах Budget. Ошибок нет.
fn ix(t: Tier) -> usize {
    match t {
        Tier::Deal => 0,
        Tier::Calc => 1,
        Tier::Background => 2,
    }
}

/// Место под поток счёта. Освобождается при Drop (в том числе у брошенного расчёта — когда его поток закончил).
pub struct Permit(());

impl Drop for Permit {
    fn drop(&mut self) {
        {
            let mut u = BUDGET.used.lock().unwrap_or_else(|p| p.into_inner());
            *u = u.saturating_sub(1);
        }
        // Будим одного ждущего самого важного яруса, у которого есть ждущие: предел яруса не меньше предела менее
        // важного (Limits::cap), поэтому если место не может взять он — не может и менее важный.
        if let Some(i) = (0..3).find(|&i| BUDGET.waiting[i].load(Ordering::Acquire) > 0) {
            BUDGET.freed[i].notify_one();
        }
    }
}

/// Счёт ждущих яруса на время ожидания (снимается и у брошенного ожидания).
struct Waiting(usize);

impl Drop for Waiting {
    fn drop(&mut self) {
        BUDGET.waiting[self.0].fetch_sub(1, Ordering::AcqRel);
    }
}

tokio::task_local! {
    /// Ярус счёта, заданный вызывающим на задачу (путь сделки — core/executor.rs; план цикла DEX ↔ DEX —
    /// core/cycle_plan.rs). Сильнее класса срочности.
    static TIER: Tier;
}

/// Что делает: работа с заданным ярусом счёта (котировки DEX внутри неё берут места этого яруса). Принимает: ярус,
/// работу. Возвращает: её итог. Ошибок нет.
pub async fn scope<F: Future>(t: Tier, work: F) -> F::Output {
    TIER.scope(t, work).await
}

/// Пределы из настроек. Принимает: хранилище настроек. Возвращает: None — cpu.threads_max не задана или 0 (предела
/// нет); резервы не заданы — 0. Ошибок нет (ошибка чтения — как «не задана»).
pub fn limits(s: &Store) -> Option<Limits> {
    let read = |n: &str| {
        s.integer(n, At::default())
            .ok()
            .and_then(|v| usize::try_from(v).ok())
    };
    let total = read("cpu.threads_max").filter(|t| *t > 0)?;
    Some(Limits {
        total,
        reserve_deal: read("cpu.reserve_deal").unwrap_or(0),
        reserve_calc: read("cpu.reserve_calc").unwrap_or(0),
    })
}

/// Ярус по признакам задачи. Принимает: класс срочности (None — не задан), признак пути сделки (финальная котировка
/// или котировка исполнения). Возвращает: ярус: путь сделки — Deal; иначе ярус, заданный вызывающим (scope); иначе
/// класс Trade — Calc, прочее — Background. Ошибок нет.
pub fn tier_of(class: Option<Urgency>, deal: bool) -> Tier {
    if deal {
        return Tier::Deal;
    }
    if let Ok(t) = TIER.try_with(|t| *t) {
        return t;
    }
    if class == Some(Urgency::Trade) {
        Tier::Calc
    } else {
        Tier::Background
    }
}

/// Взять место без ожидания. Принимает: пределы, ярус. Возвращает: место или None (мест яруса нет). Ошибок нет.
pub fn try_acquire(lim: Limits, t: Tier) -> Option<Permit> {
    let mut u = BUDGET.used.lock().unwrap_or_else(|p| p.into_inner());
    if *u < lim.cap(t) {
        *u += 1;
        Some(Permit(()))
    } else {
        None
    }
}

/// Взять место, дождавшись его. Принимает: пределы, ярус. Возвращает: место. Ошибок нет. Брошенное ожидание места не
/// занимает и снимается из ждущих. Подписка на «освободилось» — до проверки: освобождение между проверкой и ожиданием
/// не теряется (у Notify::notify_one без ждущего остаётся разрешение).
pub async fn acquire(lim: Limits, t: Tier) -> Permit {
    if let Some(p) = try_acquire(lim, t) {
        return p;
    }
    let i = ix(t);
    BUDGET.waiting[i].fetch_add(1, Ordering::AcqRel);
    let _w = Waiting(i);
    loop {
        let freed = BUDGET.freed[i].notified();
        tokio::pin!(freed);
        let _ = freed.as_mut().enable();
        if let Some(p) = try_acquire(lim, t) {
            return p;
        }
        freed.await;
    }
}

/// Сколько мест занято сейчас (для замера и тестов). Ошибок нет.
pub fn used() -> usize {
    *BUDGET.used.lock().unwrap_or_else(|p| p.into_inner())
}
