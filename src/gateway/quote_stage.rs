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
//! Кто вызывает: core/profit.rs (ставит признак, спрашивает final_external), venues/bsc_dex/venue.rs (читает в quote).
//! От чего зависит: tokio (task_local), settings/read.rs.
//! Настройки: exec.via_kyber [площадка DEX] — только чтение в final_external.

use std::future::Future;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::settings::store::Store;

tokio::task_local! {
    static FINAL: bool;
    static SETTLEMENT: ();
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

/// Идёт ли сейчас финальная котировка расчёта. Не задано — нет. Ошибок нет.
pub fn is_final() -> bool {
    FINAL.try_with(|f| *f).unwrap_or(false)
}

/// Денежный маршрут требует восстановления каждого рыночного действия по одному сохранённому ID.
pub async fn settlement_if<F: Future>(on: bool, work: F) -> F::Output {
    if on {
        SETTLEMENT.scope((), work).await
    } else {
        work.await
    }
}
pub fn settlement() -> bool {
    SETTLEMENT.try_with(|_| ()).is_ok()
}

/// Применимость собственного лимита финальных запросов KyberSwap.
pub fn kyber_final(settings: &Store, venue: &VenueId) -> bool {
    settings
        .flag(
            "exec.via_kyber",
            At {
                venue: Some(&venue.0),
                coin: None,
            },
        )
        .unwrap_or(false)
}
