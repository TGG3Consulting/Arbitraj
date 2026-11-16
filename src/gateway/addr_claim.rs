//! Файл: gateway/addr_claim.rs
//! Что делает: решение 316 (доработка по ревью) — признак задачи «без создания адреса депозита». Некоторые адаптеры на
//! запрос адреса СОЗДАЮТ его, если его нет (KuCoin — решение 208, Add Deposit Address V3; Kraken — claim при
//! venue.deposit_address_claim = да). Фоновый прогрев адресов (core/deposit_warm*.rs) не должен создавать адреса по
//! монетам, по которым сделки нет (ревью 266 сознательно не создавало адрес без сделки): прогрев выполняет запрос
//! внутри `forbid`, адаптер перед созданием спрашивает `allowed` и при запрете возвращает NotApplicable, а чтение уже
//! существующего адреса выполняет как раньше. Признак живёт на задаче tokio (task_local, как срок терпения
//! patience.rs): ядро о площадках не знает, адаптер — о ядре; признак — здесь, в шлюзе.
//! Вне признака (предпроверка, перевод сделки, ручные команды) — как раньше: создание разрешено.
//! Кто вызывает: core/deposit_warm_pass.rs (forbid — если venue.deposit_warm_create не «да»), адаптеры
//! venues/kucoin/deposit_addr.rs и venues/kraken/deposit_addr.rs (allowed — перед созданием).
//! От чего зависит: tokio (task_local).
//! Настройки: нет своих (venue.deposit_warm_create читает ядро).

use std::future::Future;

tokio::task_local! {
    /// true — работа задачи не должна создавать адреса депозита.
    static FORBID: bool;
}

/// Что делает: выполняет работу с запретом создания адреса депозита. Принимает: работу. Возвращает: её итог.
/// Ошибок нет.
pub async fn forbid<F: Future>(work: F) -> F::Output {
    FORBID.scope(true, work).await
}

/// Что делает: можно ли адаптеру создать адрес депозита в этой задаче. Вне признака — можно (как раньше).
/// Ошибок нет.
pub fn allowed() -> bool {
    !FORBID.try_with(|f| *f).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn forbid_only_inside_scope() {
        assert!(allowed(), "вне признака — можно, как раньше");
        assert!(!forbid(async { allowed() }).await);
        assert!(allowed(), "признак не протекает за пределы задачи");
    }
}
