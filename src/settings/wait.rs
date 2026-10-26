//! Файл: settings/wait.rs
//! Что делает: «настройка не задана — задали на лету — работа пошла без перезапуска» (CLAUDE.md: настройки меняются на
//! лету без перезапуска, решение 75) для любого компонента: цикл, которому нужна настройка, не останавливается
//! навсегда, а ждёт изменения настроек и пробует снова. В журнал — одна запись «ждёт настройку» и одна «снова
//! работает».
//! Кто вызывает: venues/bsc_dex/settings_wait.rs, venues/bingx/{fees, pairs_watch}.rs.
//! От чего зависит: settings/live.rs (Watch), settings/store.rs, storage/journal.rs, gateway/errors.rs.
//! Настройки: нет (ждёт любые).

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::{Journal, Level};

use super::live::Watch;
use super::store::Store;

/// Где писать о ожидании: журнал, вид записи, площадка.
pub struct Note<'a> {
    pub journal: &'a Journal,
    pub kind: &'a str,
    pub venue: Option<&'a str>,
}

/// Значение из `read`, а пока его нет (не задано или задано неверно) — одна запись «`what` ждёт настройку: причина» и
/// ожидание любого изменения настроек, затем снова; дождались после ожидания — запись «`what`: настройку задали —
/// снова работает». Принимает: хранилище настроек, куда писать, что ждёт, чтение. Возвращает: значение; None —
/// рассылка настроек закрыта (система останавливается). Ошибок нет.
pub async fn until_ok<T>(
    store: &Store,
    note: Note<'_>,
    what: &str,
    read: impl Fn() -> GatewayResult<T>,
) -> Option<T> {
    let mut waited = false;
    loop {
        // Подписка — до чтения: изменение между чтением и ожиданием не теряется.
        let mut w = Watch::new(store.subscribe(), Vec::new());
        match read() {
            Ok(v) => {
                if waited {
                    let msg = format!("{what}: настройку задали — снова работает");
                    let _ = note
                        .journal
                        .write(Level::Info, note.kind, note.venue, msg, None)
                        .await;
                }
                return Some(v);
            }
            Err(e) => {
                if !waited {
                    let msg = format!(
                        "{what} ждёт настройку: {} — продолжит сам, как только её зададут",
                        e.message
                    );
                    let _ = note
                        .journal
                        .write(Level::Error, note.kind, note.venue, msg, None)
                        .await;
                    waited = true;
                }
                w.next().await?;
            }
        }
    }
}
