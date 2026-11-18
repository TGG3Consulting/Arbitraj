//! Файл: notify/press_event_coin.rs
//! Что делает: решение 408 (t193, исключение из решения 157 — Тигран 18.11) — кнопки тревог с правкой настроек монеты
//! (вынесено из press_event.rs — размер файла): oe.<биржа>.<монета> — «Открыть вход монеты» (coin.withdraw_refused →
//! OpenCoinEntry), ub.<монета> — «Убрать из чёрного списка» (coin.sell_slip, coin.credit_shortfall → Unblacklist).
//! Отметка читается сейчас (нет — отказ «уже открыт / уже нет»), вопрос «Да, …» с причиной отметки; правку делает
//! команда ядра (core/commands_ext.rs: Store::set с автором «кто нажал», история изменений настроек).
//! Кто вызывает: notify/press_event.rs (event_button).
//! От чего зависит: press_event.rs (pending, BAD), press.rs (Ctx), settings (Store, Scope), human.rs.
//! Настройки: читает coin.entry_blocked_venues, coin.entry_blocked_note, coin.blocked, coin.blocked_note.

use crate::core::types::ManualAction;
use crate::settings::scope::Scope;

use super::mirror_state::Pending;
use super::press::Ctx;
use super::press_event::{BAD, pending};

/// «Открыть вход монеты»: биржа в coin.entry_blocked_venues монеты сейчас. Ошибки: текст для нажавшего.
pub(super) fn ev_entry(c: &Ctx<'_>, arg: &str) -> Result<Pending, String> {
    let (venue, coin) = arg.split_once('.').ok_or(BAD)?;
    let scope = Scope::Coin(coin.to_string());
    let list = c
        .settings
        .get_exact("coin.entry_blocked_venues", &scope)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    if !list.iter().any(|v| v.as_str() == Some(venue)) {
        return Err("Вход уже открыт — отметки на экране «Монеты» нет".into());
    }
    let note = c
        .settings
        .get_exact("coin.entry_blocked_note", &scope)
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "причина не указана".into());
    let q = format!(
        "Открыть входы монеты {coin} с {}? Закрыто после отказа вывода: {note}. Правка настройки \
         coin.entry_blocked_venues — как «Открыть» на экране «Монеты», в историю настроек с вашим именем.",
        super::human::venue(venue)
    );
    Ok(pending(
        ManualAction::OpenCoinEntry {
            coin: coin.into(),
            venue: venue.into(),
        },
        None,
        q,
    ))
}

/// «Убрать из чёрного списка»: монета сейчас в ручном списке (coin.blocked). Ошибки: текст для нажавшего.
pub(super) fn ev_unblock(c: &Ctx<'_>, coin: &str) -> Result<Pending, String> {
    let scope = Scope::Coin(coin.to_string());
    let on = c
        .settings
        .get_exact("coin.blocked", &scope)
        .and_then(|v| v.as_bool())
        == Some(true);
    if !on {
        return Err("Монеты уже нет в ручном чёрном списке".into());
    }
    let note = c
        .settings
        .get_exact("coin.blocked_note", &scope)
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "причина не указана".into());
    let q = format!(
        "Убрать {coin} из ручного чёрного списка? Внесена: {note}. Правка настроек coin.blocked / coin.blocked_note — \
         как «Убрать» на экране «Монеты», в историю настроек с вашим именем."
    );
    Ok(pending(
        ManualAction::Unblacklist { coin: coin.into() },
        None,
        q,
    ))
}
