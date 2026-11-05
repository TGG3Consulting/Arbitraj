//! Файл: notify/ctl/transfer_input.rs
//! Что делает: диалог «Перевести USDT» чата «Управление» (решение 176; продолжение transfer_press.rs — размер файла):
//! «Ввести сумму» — вопрос с ForceReply, ответ числом → сводка (или шаг «Сколько» с причиной); «Отправить» — сводка
//! заново (проверка ядра usdt_transfer_plan::quote), вопрос «Да, перевести USDT» с суммой, комиссией, сколько дойдёт и
//! направлением (press_confirm.rs ask) → команда ядра TransferUsdt; диалог при этом закрывается (второе «Отправить» —
//! «экран устарел»).
//! Кто вызывает: notify/ctl/transfer_press.rs (ctl_transfer), notify/ctl/press.rs (ctl_message).
//! От чего зависит: ctl/sessions.rs, ctl/transfer.rs (n), core/usdt_sources.rs, core/usdt_transfer_plan.rs,
//! press_confirm.rs (ask), tg_methods.rs (force_reply).
//! Настройки: notify.dialog_ttl_s, ui.balances_refresh_ms (через Core::balances_age).
//!
//! ForceReply: https://core.telegram.org/bots/api#forcereply

use rust_decimal::Decimal;

use crate::core::types::ManualAction;
use crate::core::usdt_sources as src;
use crate::core::usdt_transfer_check::End;
use crate::core::usdt_transfer_plan as plan;
use crate::storage::journal::Level;

use super::super::mirror_state::Pending;
use super::super::press::Ctx;
use super::super::tg_buttons::{CallbackQuery, InMessage};
use super::super::tg_methods::force_reply;
use super::kb::Screen;
use super::press::STALE;
use super::sessions::{Dialog, Sess, Step, lock};
use super::transfer::n;

impl Ctx<'_> {
    /// «Ввести сумму»: вопрос с ForceReply (ответ — transfer_typed). Ошибок нет.
    pub(super) async fn transfer_ask_amount(
        &self,
        q: &CallbackQuery,
        sid: u64,
        s: &Sess,
        d: &Dialog,
    ) {
        let w = d.withdrawable.map(n).unwrap_or_else(|| "—".into());
        let text =
            format!("Сколько USDT перевести? Можно вывести {w}. Ответьте на это сообщение числом.");
        match self
            .tg
            .send_markup(
                &s.chat.to_string(),
                &text,
                Some(force_reply("USDT")),
                Some(s.msg),
            )
            .await
        {
            Ok(id) => {
                lock(self.ctl).prompts.insert((s.chat, id), sid);
                self.answer(&q.id, Some("Введите сумму ответом на сообщение бота"))
                    .await;
            }
            Err(e) => {
                self.answer(&q.id, Some("Не удалось спросить сумму — смотрите журнал"))
                    .await;
                self.log(
                    Level::Error,
                    format!("Перевод USDT: вопрос суммы не отправлен: {e}"),
                )
                .await;
            }
        }
    }

    /// Ответ на вопрос суммы: число → сводка (или шаг «Сколько» с причиной). Ошибок нет.
    pub async fn transfer_typed(&self, sid: u64, text: &str, m: &InMessage, who: &str) {
        let got = self.ttl().ok().and_then(|t| lock(self.ctl).get(sid, t));
        let Some((s, Some(mut d))) = got.map(|s| (s.clone(), s.dialog)) else {
            return self.say(STALE, Some(m.message_id)).await;
        };
        self.log(
            Level::Info,
            format!("Telegram: {who} ввёл сумму перевода «{}»", text.trim()),
        )
        .await;
        match text.trim().replace(',', ".").parse::<Decimal>() {
            Ok(a) if a > Decimal::ZERO => (d.amount, d.step) = (Some(a), Step::Summary),
            _ => d.error = Some(format!("«{}» — не число больше нуля", text.trim())),
        }
        self.transfer_show(s.chat, Some(s.msg), d).await;
    }

    /// «Отправить»: вопрос «Да, перевести USDT» с суммой и направлением; диалог закрывается. Ошибок нет.
    pub(super) async fn transfer_go(&self, q: &CallbackQuery, sid: u64, s: &Sess, d: &Dialog) {
        let (Some(f), Some(t), Some(a)) = (&d.from, &d.to, d.amount) else {
            return self.answer(&q.id, Some(STALE)).await;
        };
        let qt = match plan::quote(self.core, f, t, Some(a)).await {
            Ok(x) => x,
            Err(e) => {
                return self
                    .answer(&q.id, Some(&format!("Перевести нельзя: {e}")))
                    .await;
            }
        };
        lock(self.ctl).sess.remove(&sid);
        let all = src::all(self.core, self.core.balances_age()).await;
        let (lf, lt) = (src::label_of(&all, f), src::label_of(&all, t));
        let fee = qt
            .fee
            .map(|x| format!("комиссия вывода {} {}", n(x), qt.ticker))
            .unwrap_or_else(|| "газ BNB с кошелька".into());
        let question = format!(
            "Перевести {} {} с {lf} на {lt}? {fee}, дойдёт {} {}, сеть BSC ({}).",
            n(qt.amount),
            qt.ticker,
            n(qt.arrives),
            qt.ticker,
            qt.network
        );
        let wallet = |e: &End| e.wallet.as_ref().map(|w| w.0.clone());
        let action = ManualAction::TransferUsdt {
            from: f.venue.0.clone(),
            from_wallet: wallet(f),
            to: t.venue.0.clone(),
            to_wallet: wallet(t),
            amount: qt.amount.to_string(),
        };
        let done = Screen::new(format!("{}\n\nПодтвердите кнопкой «Да» ниже.", s.shown));
        let _ = self.put_screen(s.chat, Some(s.msg), &done).await;
        let p = Pending {
            key: None,
            title: action.title().into(),
            action,
            question,
        };
        self.ask(&q.id, Some(s.msg), p).await;
    }
}
