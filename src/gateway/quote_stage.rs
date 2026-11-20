//! Файл: gateway/quote_stage.rs
//! Что делает: признак «финальная котировка расчёта» (решение 95) — общий для ядра и адаптеров. Ядро ищет объём
//! кандидата дешёвыми котировками (математика площадки), а найденный оптимум пересчитывает ещё раз внутри
//! `final_scope`: так адаптер узнаёт, что это котировка, по которой решается «торговать или нет», и может отдать
//! цену того, кто реально исполнит обмен (у DEX — агрегатор KyberSwap при exec.via_kyber). Ядро о площадках не знает:
//! оно только ставит признак; что с ним делать — решает адаптер. Признак задаётся на задачу tokio (task_local), как
//! класс срочности (urgency.rs); новая задача его не наследует.
//! Решение 121 B5: `final_external` — финальная котировка площадки идёт к внешнему исполнителю с лимитом запросов
//! (KyberSwap при exec.via_kyber = да). Ядро по нему решает, применять ли фильтр kyber.final_gate_usd, не зная
//! площадок и не импортируя адаптеры: имя переключателя живёт здесь, в шлюзе.
//! Кто вызывает: core/profit.rs (ставит признак, спрашивает final_external), core/ride_pair.rs (own_scope — пара
//! котировок из одного источника), venues/bsc_dex/venue.rs (читает is_final и own_only в quote, quote_fills).
//! От чего зависит: tokio (task_local), settings/read.rs.
//! Настройки: exec.via_kyber [площадка DEX] — только чтение в final_external.

use std::future::Future;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::settings::store::Store;

tokio::task_local! {
    static FINAL: bool;
    /// Решение 480 (ревью 4): только своя математика площадки (own_scope).
    static OWN: bool;
}

/// Выполняет работу как финальную котировку расчёта. Принимает: работу. Возвращает: её результат. Ошибок нет.
pub async fn final_scope<F: Future>(work: F) -> F::Output {
    FINAL.scope(true, work).await
}

/// Как `final_scope`, но только если `on` (решение 129: перенос признака в поток счёта котировки DEX,
/// venues/bsc_dex/quote/cpu.rs). Ошибок нет.
pub async fn final_if<F: Future>(on: bool, work: F) -> F::Output {
    if on {
        FINAL.scope(true, work).await
    } else {
        work.await
    }
}

/// Решение 480 (ревью 4): выполняет работу как котировку СВОЕЙ математикой площадки — адаптер не обращается к
/// внешнему исполнителю (у DEX — KyberSwap при exec.via_kyber и «лучший из двух»), даже внутри final_scope; признак
/// финальной котировки (is_final — выбор исполнимого маршрута, ярус счёта) не меняется. Нужно, когда две котировки
/// должны быть из одного источника (пара котировок marginal, core/ride_pair.rs). Принимает: работу. Возвращает: её
/// результат. Ошибок нет.
pub async fn own_scope<F: Future>(work: F) -> F::Output {
    OWN.scope(true, work).await
}

/// Решение 480 (ревью 4): идёт ли котировка только своей математикой (own_scope). Не задано — нет. Ошибок нет.
pub fn own_only() -> bool {
    OWN.try_with(|o| *o).unwrap_or(false)
}

/// Идёт ли сейчас финальная котировка расчёта. Не задано — нет. Ошибок нет.
pub fn is_final() -> bool {
    FINAL.try_with(|f| *f).unwrap_or(false)
}

/// Финальная котировка площадки — у внешнего исполнителя с лимитом запросов (решения 95, 111, 121 B5): exec.via_kyber
/// = да в области площадки (тот же переключатель, что читает её адаптер). Принимает: хранилище настроек, площадку.
/// Возвращает: да/нет; не задано или ошибка чтения — нет (своя котировка, внешних запросов нет). Ошибок нет.
pub fn final_external(settings: &Store, venue: &VenueId) -> bool {
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    settings.flag("exec.via_kyber", at).unwrap_or(false)
}
