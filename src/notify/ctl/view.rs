//! Файл: notify/ctl/view.rs
//! Что делает: собирает экран чата «Управление» (решение 176) по тому, что на нём (View), из данных ядра: «Состояние» —
//! из памяти ядра (Core::control), «Балансы» — из памяти панели «Балансы» (Core::venue_balances / wallet_balances с
//! допустимым возрастом ответа), «Ручное управление» — из записей экрана и памяти зеркала. Общая сборка у нажатий
//! (ctl/press.rs) и лайв-обновления (ctl/live.rs) — одна логика.
//! Кто вызывает: notify/ctl/press.rs, notify/ctl/live.rs.
//! От чего зависит: core (control, manual_records, venue_balances, wallet_balances, cex_ids), notify/mirror_state.rs,
//! notify/roles.rs (чат зеркала), ctl/screens.rs, ctl/balances.rs, ctl/sessions.rs, settings (accounts).
//! Настройки: ID чата зеркала (roles::mirror_role) — для ссылки «Открыть в чате».

use std::time::Duration;

use crate::core::Core;
use crate::core::types::VenueId;
use crate::settings::store::Store;

use super::super::mirror_state::{Shared, lock};
use super::super::roles;
use super::kb::Screen;
use super::sessions::{Ref, View};
use super::{balances, screens};

/// Экран по виду. Принимает: ядро, настройки, память зеркала, вид, номер отрисовки (для денежных кнопок), допустимый
/// возраст балансов (None — спросить площадку сейчас). Возвращает: (экран, денежные кнопки). Ошибок нет: сбой — в тексте.
pub async fn render(
    core: &Core,
    settings: &Store,
    mirror: &Shared,
    view: &View,
    sid: u64,
    age: Option<Duration>,
) -> (Screen, Vec<Ref>) {
    match view {
        View::Root | View::Transfer => (screens::root(&core.control()), vec![]),
        View::State => {
            let recs = core.manual_records();
            let n = lock(mirror).waiting(&recs).len();
            (screens::state(&core.control(), n), vec![])
        }
        View::Balances => {
            let mut w = settings.accounts().wallets();
            w.sort_by(|a, b| a.name.cmp(&b.name));
            (balances::list(&core.cex_ids(), &w), vec![])
        }
        View::Venue(v) => match core.venue_balances(&VenueId(v.clone()), age).await {
            Ok(b) => balances::venue(&b, sid),
            Err(e) => (
                Screen::new(format!("Балансы {v}: {e}")).nav(Some("k:bal")),
                vec![],
            ),
        },
        View::Wallet(id) => {
            let title = settings
                .accounts()
                .wallet(id)
                .map(|w| w.name)
                .unwrap_or_else(|| id.clone());
            match core.wallet_balances(id, age).await {
                Ok(w) => (balances::wallet(&w, &title), vec![]),
                Err(e) => (
                    Screen::new(format!("Кошелёк {title}: {e}")).nav(Some("k:bal")),
                    vec![],
                ),
            }
        }
        View::Manual => (manual(core, settings, mirror), vec![]),
    }
}

/// «Ручное управление»: ждущие записи и ссылка на последнее сообщение зеркала. Ошибок нет.
fn manual(core: &Core, settings: &Store, mirror: &Shared) -> Screen {
    let recs = core.manual_records();
    let st = lock(mirror);
    let waiting = st.waiting(&recs);
    let role = roles::mirror_role(settings);
    let chat = roles::chat(settings, role).and_then(|c| c.parse::<i64>().ok());
    let last = st
        .reminder
        .or_else(|| st.sent.values().flat_map(|s| s.msgs.iter().copied()).max());
    let link = chat
        .zip(last)
        .and_then(|(c, m)| screens::message_link(c, m));
    screens::manual(&waiting, role.title(), link)
}

/// Вид по коду «k:<экран>». Принимает: код. Возвращает: вид или None — нет такого. Ошибок нет.
pub fn parse(code: &str) -> Option<View> {
    Some(match code.split_once('.') {
        Some(("bv", v)) if !v.is_empty() => View::Venue(v.into()),
        Some(("bw", w)) if !w.is_empty() => View::Wallet(w.into()),
        Some(_) => return None,
        None => match code {
            "root" => View::Root,
            "st" => View::State,
            "bal" => View::Balances,
            "man" => View::Manual,
            "tr" => View::Transfer,
            _ => return None,
        },
    })
}
