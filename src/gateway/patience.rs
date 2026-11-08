//! Файл: gateway/patience.rs
//! Что делает: решение 201 (Тигран, 06.11.2026: «Продать сейчас» RWA на Bitget отклонялась «пауза после HTTP 429 — ещё
//! 828 мс») — «терпение» ручной работы: пока площадка выжидает паузу после 429 (своя пауза адаптера, урок 15), запрос
//! ручной команды или продажи сделки не отказывает сразу, а ждёт конец паузы — не дольше срока терпения. Срок задаёт
//! ядро на задачу tokio (task_local, как класс срочности urgency.rs и признак финальной котировки quote_stage.rs);
//! адаптер перед каждым запросом REST спрашивает `wait` с остатком своей паузы. Ядро о площадках не знает, адаптер —
//! о ядре: признак живёт здесь, в шлюзе. Вне срока (автоматика: расчёты, опросы) — как раньше, отказ сразу.
//! Тот же срок — ожидание загрузки справочника пар после включения биржи (`deadline_left`, адаптер Bitget).
//! Второй признак — `exec_scope`: котировка для исполнения (оценка продажи перед ордером), а не для потока расчётов.
//! Адаптер может по нему взять свежий снимок стакана по REST, если стакан потока пары тихий или ещё не пришёл (Bitget:
//! неликвидная пара не шлёт кадров без изменений — venues/bitget/rest_book.rs). Поток расчётов снимком не пользуется.
//! Кто вызывает: core (scope — ручные команды, продажа сделки; exec_scope — trade_ops::sell_value у DEX и exec_book.rs —
//! стакан биржи для ордера, решение 237), адаптеры (wait — перед запросом REST; is_exec — котировка и снимок Bitget).
//! От чего зависит: tokio (task_local, time).
//! Настройки: нет своих (срок — exec.rate_limit_wait_max_ms читает ядро, core/patience_scope.rs).

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

tokio::task_local! {
    /// Крайний момент, до которого работа задачи может ждать паузу площадки.
    static DEADLINE: Instant;
    /// true — котировка для исполнения (оценка продажи перед ордером).
    static EXEC: bool;
}

/// Выполняет работу как котировку для исполнения. Принимает: работу. Возвращает: её итог. Ошибок нет.
pub async fn exec_scope<F: Future>(work: F) -> F::Output {
    EXEC.scope(true, work).await
}

/// Идёт ли котировка для исполнения. Вне признака — нет. Ошибок нет.
pub fn is_exec() -> bool {
    EXEC.try_with(|e| *e).unwrap_or(false)
}

/// Выполняет работу с терпением `max` (от текущего момента). Принимает: срок, работу. Возвращает: её итог. Ошибок нет.
pub async fn scope<F: Future>(max: Duration, work: F) -> F::Output {
    DEADLINE.scope(Instant::now() + max, work).await
}

/// Сколько ещё можно ждать в этой задаче. None — терпения нет (автоматика) или срок вышел. Ошибок нет.
pub fn deadline_left() -> Option<Duration> {
    let d = DEADLINE.try_with(|d| *d).ok()?;
    let now = Instant::now();
    (d > now).then(|| d - now)
}

/// Ждёт конец паузы площадки, если у задачи хватает терпения. Принимает: остаток паузы, мс (≤ 0 — паузы нет).
/// Возвращает: true — подождали (пауза кончилась), false — не ждали (паузы нет, терпения нет или его не хватает: тогда
/// адаптер отказывает, как раньше). Ошибок нет.
pub async fn wait(left_ms: i64) -> bool {
    if left_ms <= 0 {
        return false;
    }
    let need = Duration::from_millis(left_ms as u64);
    match deadline_left() {
        Some(have) if have >= need => {
            tokio::time::sleep(need).await;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn waits_only_with_patience() {
        assert!(!wait(5).await, "без терпения — не ждём");
        assert!(scope(Duration::from_secs(1), wait(5)).await);
        assert!(
            !scope(Duration::from_millis(1), wait(500)).await,
            "терпения не хватает"
        );
        assert!(!scope(Duration::from_secs(1), wait(0)).await, "паузы нет");
        assert!(!is_exec());
        assert!(exec_scope(async { is_exec() }).await);
    }
}
