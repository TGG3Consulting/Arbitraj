//! Файл: venues/bsc_dex/quote/abandon.rs
//! Что делает: признак «расчёт брошен» для счёта котировки DEX вне рабочих потоков tokio (решение 301, часть в; задача
//! t132). Счёт идёт в потоке блокирующих задач (quote/cpu.rs: spawn_blocking + block_on); когда вызывающий бросает
//! ожидание (срок расчёта profit.calc_timeout_ms вышел — tokio::time::timeout ядра бросает future), сам поток счёта не
//! останавливается. Раньше брошенный счёт досчитывал всю работу, включая ожидание замка построения графа и своё
//! построение, — занимал потоки и замок, пока новые расчёты уже шли (самоподдержка перегрузки). Теперь cpu::run ставит
//! на счёт признак (task_local, как класс срочности urgency.rs) и поднимает его, когда вызывающий бросил ожидание
//! (Guard::drop); тяжёлые шаги проверяют признак перед собой (check) и выходят сразу. Чисел нет — только флаг.
//! Вне cpu::run (тесты, другие вызовы) признака нет — check всегда пропускает.
//! Кто вызывает: quote/cpu.rs (guard, scope); pools/graph_cache.rs (до ожидания замка построения и перед построением),
//! pools/graph.rs (на каждом пуле построения), quote/graph_route.rs (перед отбором лучших путей, сплитом и
//! повтором графа решения 300),
//! quote/graph_route.rs failed (брошенный расчёт не пишет в журнал «граф-роутер не дал маршрут»).
//! От чего зависит: ctx.rs, gateway/errors.rs, tokio (task_local).
//! Настройки: нет.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;

tokio::task_local! {
    /// Признак «расчёт брошен» текущего счёта.
    static FLAG: Arc<AtomicBool>;
}

/// Поднимает признак, когда его владелец (ожидание вызывающего в cpu::run) брошен. После обычного завершения счёта
/// признак тоже поднимается — его уже никто не читает.
pub struct Guard(Arc<AtomicBool>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Новый признак: страж (держит вызывающий) и сам признак (уходит в счёт через scope). Ошибок нет.
pub fn guard() -> (Guard, Arc<AtomicBool>) {
    let f = Arc::new(AtomicBool::new(false));
    (Guard(f.clone()), f)
}

/// Ставит признак на работу `f`. Принимает: признак, работу. Возвращает: её итог.
pub async fn scope<F: Future>(flag: Arc<AtomicBool>, f: F) -> F::Output {
    FLAG.scope(flag, f).await
}

/// Брошен ли текущий расчёт. Вне scope — нет. Ошибок нет.
pub fn is_set() -> bool {
    FLAG.try_with(|f| f.load(Ordering::Acquire))
        .unwrap_or(false)
}

/// Проверка перед тяжёлым шагом. Принимает: окружение (текст ошибки), шаг (для текста). Возвращает: Ok — расчёт жив.
/// Ошибки: Timeout — расчёт брошен (срок расчёта вышел); итог брошенного счёта никто не читает.
pub fn check(ctx: &Ctx, step: &str) -> GatewayResult<()> {
    if is_set() {
        return Err(ctx.err(
            ErrorCode::Timeout,
            format!("расчёт брошен (срок расчёта вышел) — {step} не выполняется"),
        ));
    }
    Ok(())
}
