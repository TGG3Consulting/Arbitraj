//! Файл: gateway/addr_claim.rs
//! Что делает: решение 316 (доработка по ревью) — признак задачи «без создания адреса депозита». Некоторые адаптеры на
//! запрос адреса СОЗДАЮТ его, если его нет (KuCoin — решение 208, Add Deposit Address V3; Kraken — claim, решение
//! 379). Фоновый прогрев адресов (core/deposit_warm*.rs) не должен создавать адреса по
//! монетам, по которым сделки нет (ревью 266 сознательно не создавало адрес без сделки): прогрев выполняет запрос
//! внутри `forbid`, адаптер перед созданием спрашивает `allowed` и при запрете возвращает NotApplicable, а чтение уже
//! существующего адреса выполняет как раньше. Признак живёт на задаче tokio (task_local, как срок терпения
//! patience.rs): ядро о площадках не знает, адаптер — о ядре; признак — здесь, в шлюзе.
//! Вне признака (предпроверка, перевод сделки, ручные команды) — как раньше: создание разрешено.
//! Решение 379 (задача t165, Тигран 17.11: «на всех биржах этот адрес должен автоматом создаваться»): адреса нет —
//! адаптер биржи с официальным методом создания (KuCoin, Poloniex, Kraken) создаёт его сам; что делать, решает
//! `on_missing` (чистая логика `decide`, тест ниже): создать или ответить NotApplicable (прогрев без «да»).
//! Кто вызывает: core/deposit_warm_pass.rs (forbid — если venue.deposit_warm_create не «да»), адаптеры
//! venues/kucoin/deposit_addr.rs (allowed — перед созданием), venues/poloniex/deposit_addr.rs и
//! venues/kraken/deposit_addr.rs (on_missing — решение 379).
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

/// Решение 379: что делать адаптеру, у биржи которого есть официальный метод создания, когда адреса в ответе нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// Создать адрес методом биржи (предпроверка, перевод сделки, ручные команды, прогрев с «да»).
    Create,
    /// Не создавать: задача под запретом (прогрев адресов без venue.deposit_warm_create = да) — NotApplicable.
    Forbidden,
}

/// Что делает: решение 379 — чистая логика выбора при отсутствии адреса. Принимает: можно ли создавать в задаче
/// (allowed). Возвращает: Missing. Ошибок нет.
pub fn decide(allowed: bool) -> Missing {
    if allowed {
        Missing::Create
    } else {
        Missing::Forbidden
    }
}

/// Что делает: решение 379 — выбор для текущей задачи (decide(allowed())). Ошибок нет.
pub fn on_missing() -> Missing {
    decide(allowed())
}

/// Что делает: текст отказа «создание запрещено» для адаптера (одинаковый у всех бирж). Принимает: площадку, монету,
/// сеть. Возвращает: текст. Ошибок нет.
pub fn forbidden_text(venue: &str, ticker: &str, network: &str) -> String {
    format!(
        "Адреса депозита {venue} {ticker} в сети {network} нет, а создание адреса в этой задаче запрещено \
         (прогрев адресов, venue.deposit_warm_create не «да»)"
    )
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

    /// Решение 379: адреса нет — вне признака создаём, под признаком (прогрев без «да») — нет.
    #[test]
    fn decide_create_or_forbidden() {
        assert_eq!(decide(true), Missing::Create);
        assert_eq!(decide(false), Missing::Forbidden);
    }

    /// Решение 379: on_missing — по признаку задачи: предпроверка создаёт, прогрев без «да» — нет.
    #[tokio::test]
    async fn on_missing_follows_scope() {
        assert_eq!(on_missing(), Missing::Create, "предпроверка — создать");
        assert_eq!(forbid(async { on_missing() }).await, Missing::Forbidden);
        assert_eq!(on_missing(), Missing::Create);
    }
}
