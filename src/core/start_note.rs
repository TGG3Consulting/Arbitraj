//! Файл: core/start_note.rs
//! Что делает: задача t165 (разбор 17.11) — итог старта прошедшего кандидата виден у сигнала. Живой случай 17.11: ESIM
//! bsc_dex → poloniex — сигналы шли, а отказ повторялся с тем же текстом и новой записи журнала не давал (core.precheck
//! — счётчик старой строки, core.start_refused — «тишина навсегда» до Started): «сигнал есть, сделки нет» выглядело как
//! тишина. Теперь: (1) текст итога старта дописывается к записи core.candidate (карточка: card_job.rs; эпизоды:
//! episode_worker.rs → episode_notify.rs); (2) core.start_refused пишется тем же механизмом повторов, что core.precheck
//! (reject_log.rs, ключ start|направление: при journal.repeat_flush_ms — счётчик и время последнего повтора растут;
//! сделка запущена — строка закрывается итоговым счётчиком).
//! Кто вызывает: core/candidates.rs (accepted_timed, refused_log), тесты (ниже).
//! От чего зависит: executor.rs (Start), candidates.rs (reject_key), core/types.
//! Настройки: нет своих (journal.repeat_flush_ms — через candidates::flush_every).

use crate::core::types::Direction;

use super::executor::Start;

/// Что делает: текст отказа исполнителя на входе (решение 147) — для поля start строки «Кандидатов» и журнала
/// core.start_refused. Принимает: итог входа. Возвращает: текст или None (не отказ: запущена, сухой режим). Ошибок нет.
pub fn refused(start: &Start) -> Option<String> {
    match start {
        Start::NotTradable(why) => Some(format!("сделки нет — направление не торгуемо: {why}")),
        Start::Busy(why) => Some(format!("сделки нет — по монете занято: {why}")),
        Start::Started | Start::Dry(_) => None,
    }
}

/// Что делает: итог старта для записи core.candidate (строка под карточкой сигнала).
/// Принимает: итог входа. Возвращает: текст — всегда (запущена, сухой режим, отказ). Ошибок нет.
pub fn note(start: &Start) -> String {
    match start {
        Start::Started => {
            "Итог старта: сделка запущена (отказ предпроверки, если будет, — журнал core.precheck)"
                .into()
        }
        Start::Dry(why) => format!("Итог старта: {why}"),
        Start::NotTradable(_) | Start::Busy(_) => {
            format!(
                "Итог старта: отказ — {}",
                refused(start).unwrap_or_default()
            )
        }
    }
}

/// Что делает: ключ строки core.start_refused в памяти повторов (reject_log.rs) — отдельный от core.candidate_rejected
/// и core.precheck того же направления. Принимает: направление. Возвращает: ключ. Ошибок нет.
pub fn key(d: &Direction) -> String {
    format!("start|{}", super::candidates::reject_key(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_only_for_refusals() {
        assert_eq!(refused(&Start::Started), None);
        assert_eq!(refused(&Start::Dry("сухой".into())), None);
        assert_eq!(
            refused(&Start::Busy("сделка 1".into())).as_deref(),
            Some("сделки нет — по монете занято: сделка 1")
        );
        assert_eq!(
            refused(&Start::NotTradable("в чёрном списке".into())).as_deref(),
            Some("сделки нет — направление не торгуемо: в чёрном списке")
        );
    }

    #[test]
    fn note_always_names_outcome() {
        assert!(note(&Start::Started).starts_with("Итог старта: сделка запущена"));
        assert_eq!(
            note(&Start::Dry("сухой режим".into())),
            "Итог старта: сухой режим"
        );
        assert_eq!(
            note(&Start::Busy("сделка 1".into())),
            "Итог старта: отказ — сделки нет — по монете занято: сделка 1"
        );
    }

    #[test]
    fn key_differs_from_candidate_and_precheck() {
        use crate::core::types::{Asset, VenueId};
        let d = Direction {
            asset: Asset {
                ticker: "ESIM".into(),
                bsc_contract: None,
            },
            buy_on: VenueId("bsc_dex".into()),
            sell_on: VenueId("poloniex".into()),
        };
        let k = key(&d);
        assert!(k.starts_with("start|"));
        assert_ne!(k, super::super::candidates::reject_key(&d));
        assert_ne!(
            k,
            format!("precheck|{}", super::super::candidates::reject_key(&d))
        );
    }
}
