//! Файл: notify/ctl/sell.rs
//! Что делает: «s:<номер>.<строка>» чата «Управление» (решение 176; вынесено из press.rs — размер файла) — «Продать»
//! монету из экрана балансов биржи: доступность — заново из памяти панели «Балансы» (не старше ui.balances_refresh_ms,
//! иначе запрос; решение 174 — та же причина, что в панели), затем вопрос «Да, продать …» с количеством, монетой,
//! счётом и биржей (press_confirm.rs ask) → та же команда ядра SellBalance, что у кнопки панели.
//! Кто вызывает: notify/press.rs.
//! От чего зависит: ctl/press.rs (ctl_get, STALE), ctl/sessions.rs (Ref), ctl/transfer.rs (n), core (venue_balances),
//! press_confirm.rs (ask), notify/human.rs.
//! Настройки: notify.dialog_ttl_s, ui.balances_refresh_ms.

use crate::core::types::{ManualAction, VenueId};

use super::super::mirror_state::Pending;
use super::super::press::Ctx;
use super::super::tg_buttons::{CallbackQuery, CbMessage};
use super::press::STALE;
use super::sessions::Ref;

impl Ctx<'_> {
    /// «s:<номер>.<строка>» — «Продать» монету из «Балансов». Ошибок нет.
    pub async fn ctl_sell(&self, q: &CallbackQuery, msg: &CbMessage, object: &str) {
        let (_, sess, rest) = match self.ctl_get(object) {
            Ok(x) => x,
            Err(why) => return self.answer(&q.id, Some(&why)).await,
        };
        let Some(Ref::Sell {
            venue,
            account,
            asset,
            ..
        }) = rest
            .parse::<usize>()
            .ok()
            .and_then(|i| sess.refs.get(i).cloned())
        else {
            return self.answer(&q.id, Some(STALE)).await;
        };
        // Доступность и количество — сейчас (из памяти панели не старше ui.balances_refresh_ms, иначе запрос).
        let b = self
            .core
            .venue_balances(&VenueId(venue.clone()), self.core.balances_age())
            .await;
        let row = b.ok().and_then(|b| {
            b.accounts
                .into_iter()
                .find(|a| a.account == account)?
                .rows
                .into_iter()
                .find(|r| r.asset == asset)
        });
        let Some(row) = row else {
            return self
                .answer(
                    &q.id,
                    Some("Монеты на счёте больше нет — нажмите «Обновить»"),
                )
                .await;
        };
        if let Some(Err(why)) = &row.sell {
            return self
                .answer(&q.id, Some(&format!("«Продать» недоступна: {why}")))
                .await;
        }
        let name = super::super::human::venue(&venue);
        let question = format!(
            "Продать {} {asset} со счёта «{account}» на {name} по рынку? Всё свободное — в USDT, выручка — на торговый счёт.",
            super::transfer::n(row.free)
        );
        let action = ManualAction::SellBalance {
            venue,
            account,
            asset,
        };
        let p = Pending {
            key: None,
            title: action.title().into(),
            action,
            question,
        };
        self.ask(&q.id, Some(msg.message_id), p).await;
    }
}
