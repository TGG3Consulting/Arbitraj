//! Файл: gateway/cpu_budget.rs
//! Что делает: решение 336 (задача t157) — общий предел потоков тяжёлого счёта процесса с приоритетом: путь сделки выше
//! расчёта кандидатов, кандидаты выше фона. Тяжёлый счёт — котировка DEX вне рабочих потоков tokio (адаптер DEX,
//! quote/cpu.rs) и её вспомогательные потоки (отбор путей, сплит, сборка графа, поиск циклов — quote/par.rs). Каждый
//! такой поток держит одно место. Мест — cpu.threads_max; из них cpu.reserve_deal берёт только путь сделки, а
//! cpu.reserve_calc — только путь сделки и кандидаты (фон их не берёт). Так под приём WebSocket, таймеры, узел и
//! исполнение остаются ядра (рабочие потоки tokio вне предела), а фон не вытесняет кандидатов, кандидаты — сделку.
//! Ярусы (Tier) по признакам задачи шлюза: финальная котировка (quote_stage) или котировка исполнения (patience) —
//! путь сделки; класс срочности Trade (urgency) — кандидат; остальное (живой экран, проверки, фон) — фон.
//! Ожидание места — внутри срока расчёта (profit.calc_timeout_ms): брошенный расчёт бросает и ожидание (место не
//! занимает). Смена настроек на лету: занятые места доживают, новые — по новым числам.
//! Не задана cpu.threads_max или 0 — общего предела нет (как до 336).
//! Кто вызывает: venues/bsc_dex/quote/cpu.rs (acquire), venues/bsc_dex/quote/par.rs (try_acquire), их тесты.
//! От чего зависит: settings (чтение), gateway/urgency.rs (класс), tokio (Notify).
//! Настройки: cpu.threads_max, cpu.reserve_deal, cpu.reserve_calc (вся система; необязательные).

use std::sync::Mutex;

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

/// Занятые места и сигнал «место освободилось». Один на процесс.
struct Budget {
    used: Mutex<usize>,
    freed: Notify,
}

static BUDGET: Budget = Budget {
    used: Mutex::new(0),
    freed: Notify::const_new(),
};

/// Место под поток счёта. Освобождается при Drop (в том числе у брошенного расчёта — когда его поток закончил).
pub struct Permit(());

impl Drop for Permit {
    fn drop(&mut self) {
        {
            let mut u = BUDGET.used.lock().unwrap_or_else(|p| p.into_inner());
            *u = u.saturating_sub(1);
        }
        BUDGET.freed.notify_waiters();
    }
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
/// или котировка исполнения). Возвращает: ярус. Ошибок нет.
pub fn tier_of(class: Option<Urgency>, deal: bool) -> Tier {
    if deal {
        Tier::Deal
    } else if class == Some(Urgency::Trade) {
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
/// занимает. Подписка на «освободилось» — до проверки: освобождение между проверкой и ожиданием не теряется.
pub async fn acquire(lim: Limits, t: Tier) -> Permit {
    loop {
        let freed = BUDGET.freed.notified();
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
