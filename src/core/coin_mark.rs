//! Файл: core/coin_mark.rs
//! Что делает: решение 355 (ревью t158 Б1) — монета в ручной чёрный список ОТМЕТКОЙ ЯДРА: coin.blocked_note и
//! coin.blocked ставятся в память настроек сразу (Store::mark_now, settings/store_mark.rs) — их видят кандидаты,
//! предпроверка и исполнитель (blacklist.rs) с этого же мгновения, до снятия замка монеты сделкой; в базу — своей
//! задачей хранилища. Тот же механизм, что ручной список (область «монета» по адресу, blacklist::coin_scope).
//! Кто вызывает: core/sell_slip.rs, core/credit_shortfall.rs, core/exit_gap.rs.
//! От чего зависит: core/blacklist.rs (coin_scope), settings/store_mark.rs.
//! Настройки: пишет coin.blocked, coin.blocked_note.

use serde_json::Value;

use crate::core::types::Asset;
use crate::settings::store::{SetError, Store};

/// Что делает: монета — в ручной чёрный список сейчас (заметка, затем отметка). Принимает: настройки, монету,
/// заметку, кто ставит. Возвращает: Ok — отметка действует. Ошибки: настройка отказала (неверное значение, копия на
/// момент входа) — текст для журнала.
pub fn block_now(s: &Store, asset: &Asset, note: &str, by: &str) -> Result<(), SetError> {
    let scope = super::blacklist::coin_scope(asset);
    let note = Value::String(note.to_string());
    s.mark_now("coin.blocked_note", scope.clone(), |_| Some(note), by)?;
    s.mark_now("coin.blocked", scope, |_| Some(Value::Bool(true)), by)?;
    Ok(())
}
