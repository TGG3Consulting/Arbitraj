//! Файл: notify/ctl/transfer_press.rs
//! Что делает: диалог «Перевести USDT» чата «Управление» (решение 176) — шаги «Откуда» → «Куда» → «Сколько» →
//! сводка в ОДНОМ сообщении (правится на месте). Каждый шаг перерисовывается с новым номером отрисовки (прежние кнопки
//! — «экран устарел»). Сумма: «Всё», доли notify.transfer_pct, «Ввести сумму» (ForceReply; число, ≤ можно вывести,
//! ≥ минимума вывода — проверка ядра usdt_transfer_plan::quote, одна у Telegram, интерфейса и команды). «Отправить» —
//! вопрос «Да, перевести USDT» с суммой, комиссией и направлением (press_confirm.rs ask) → команда ядра TransferUsdt;
//! диалог при этом закрывается (повторное «Отправить» — «экран устарел»).
//! «Ввести сумму», ответ числом и «Отправить» — transfer_input.rs.
//! Кто вызывает: notify/ctl/press.rs, notify/press.rs (t:…).
//! От чего зависит: ctl/transfer.rs (рендер), ctl/sessions.rs, core/usdt_sources.rs, core/usdt_transfer_plan.rs,
//! press_confirm.rs (ask), tg_methods.rs (force_reply).
//! Настройки: notify.dialog_ttl_s, notify.transfer_pct, ui.balances_refresh_ms (через Core::balances_age).
//!
//! ForceReply: https://core.telegram.org/bots/api#forcereply

use std::time::Instant;

use rust_decimal::Decimal;

use crate::core::usdt_sources as src;
use crate::core::usdt_transfer_check::End;
use crate::core::usdt_transfer_plan as plan;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::press::Ctx;
use super::super::tg_buttons::{CallbackQuery, CbMessage};
use super::kb::Screen;
use super::press::STALE;
use super::sessions::{Dialog, Ref, Sess, Step, View, lock};
use super::transfer as tr;

impl Ctx<'_> {
    /// Начало диалога: шаг «Откуда». Принимает: чат, сообщение для правки (None — новое). Ошибок нет.
    pub async fn transfer_start(&self, chat: i64, msg: Option<i64>) {
        let d = Dialog {
            step: Step::From,
            from: None,
            to: None,
            withdrawable: None,
            amount: None,
            error: None,
        };
        self.transfer_show(chat, msg, d).await;
    }

    /// Рисует шаг диалога и запоминает отрисовку. Ошибок нет.
    pub(super) async fn transfer_show(&self, chat: i64, msg: Option<i64>, mut d: Dialog) {
        let sid = lock(self.ctl).reserve();
        let all = src::all(self.core, self.core.balances_age()).await;
        let label = |e: &Option<End>| {
            e.as_ref()
                .map(|e| src::label_of(&all, e))
                .unwrap_or_default()
        };
        let route = (label(&d.from), label(&d.to));
        let err = d.error.take();
        let (screen, refs) = match (d.step, &d.from, &d.to) {
            (Step::To, Some(f), _) => tr::to(&route.0, &src::to_list(&all, f), sid),
            (Step::Amount, Some(f), Some(t)) => {
                let q = plan::quote(self.core, f, t, None).await;
                d.withdrawable = q.as_ref().ok().map(|q| q.withdrawable);
                tr::amount(
                    (&route.0, &route.1),
                    q.as_ref().map_err(|e| e.as_str()),
                    &self.pcts().await,
                    sid,
                    err.as_deref(),
                )
            }
            (Step::Summary, Some(f), Some(t)) => match plan::quote(self.core, f, t, d.amount).await
            {
                Ok(q) => (tr::summary((&route.0, &route.1), &q, sid), vec![]),
                Err(e) => {
                    d.step = Step::Amount;
                    d.error = Some(e);
                    return Box::pin(self.transfer_show(chat, msg, d)).await;
                }
            },
            _ => tr::from(&all, &src::from_list(&all), sid, err.as_deref()),
        };
        let Some(msg) = self.put_screen(chat, msg, &screen).await else {
            return;
        };
        let s = Sess {
            chat,
            msg,
            view: View::Transfer,
            refs,
            dialog: Some(d),
            touched: Instant::now(),
            shown: screen.text,
            frozen: false,
            edited: None,
        };
        lock(self.ctl).put(sid, s);
    }

    /// Доли «можно вывести» из notify.transfer_pct (%, больше 0 и не больше 100). Не задана — только «Всё» и ввод.
    /// Неверное значение — в журнал и пропускается. Ошибок нет.
    async fn pcts(&self) -> Vec<Decimal> {
        let list = self
            .settings
            .list("notify.transfer_pct", At::default())
            .unwrap_or_default();
        let mut out = Vec::new();
        for x in list {
            match x.trim().replace(',', ".").parse::<Decimal>() {
                Ok(p) if p > Decimal::ZERO && p <= Decimal::ONE_HUNDRED => out.push(p),
                _ => {
                    self.log(
                        Level::Warn,
                        format!("notify.transfer_pct: «{x}» — не доля от 0 до 100 %, пропущено"),
                    )
                    .await
                }
            }
        }
        out
    }

    /// «t:<номер>.<действие>» — шаг диалога. Ошибок нет.
    pub async fn ctl_transfer(&self, q: &CallbackQuery, msg: &CbMessage, object: &str) {
        let (sid, s, op) = match self.ctl_get(object) {
            Ok(x) => x,
            Err(why) => return self.answer(&q.id, Some(&why)).await,
        };
        let Some(mut d) = s.dialog.clone() else {
            return self.answer(&q.id, Some(STALE)).await;
        };
        let pick = |p: &str| {
            op.strip_prefix(p)
                .and_then(|i| i.parse::<usize>().ok())
                .and_then(|i| s.refs.get(i).cloned())
        };
        let (chat, m) = (msg.chat.id, Some(msg.message_id));
        match (d.step, op) {
            (Step::From, _) if op.starts_with('f') => match pick("f") {
                Some(Ref::End(e)) => (d.from, d.step) = (Some(e), Step::To),
                _ => return self.answer(&q.id, Some(STALE)).await,
            },
            (Step::To, _) if op.starts_with('d') => match pick("d") {
                Some(Ref::End(e)) => (d.to, d.step) = (Some(e), Step::Amount),
                _ => return self.answer(&q.id, Some(STALE)).await,
            },
            (Step::Amount, "a") => (d.amount, d.step) = (d.withdrawable, Step::Summary),
            (Step::Amount, _) if op.starts_with('p') => match (pick("p"), d.withdrawable) {
                (Some(Ref::Pct(p)), Some(w)) => {
                    (d.amount, d.step) = (Some(w * p / Decimal::ONE_HUNDRED), Step::Summary)
                }
                _ => return self.answer(&q.id, Some(STALE)).await,
            },
            (Step::Amount, "e") => return self.transfer_ask_amount(q, sid, &s, &d).await,
            (Step::Summary, "go") => return self.transfer_go(q, sid, &s, &d).await,
            (_, "x") => {
                lock(self.ctl).sess.remove(&sid);
                self.answer(&q.id, Some("Перевод отменён")).await;
                let done = Screen::new("Перевод USDT отменён.").nav(None);
                let _ = self.put_screen(chat, m, &done).await;
                return;
            }
            (_, "b") => {
                d.step = match d.step {
                    Step::Summary => Step::Amount,
                    Step::Amount => Step::To,
                    _ => Step::From,
                }
            }
            _ => return self.answer(&q.id, Some(STALE)).await,
        }
        self.answer(&q.id, None).await;
        self.transfer_show(chat, m, d).await;
    }
}
