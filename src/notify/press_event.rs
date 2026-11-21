//! Файл: notify/press_event.rs
//! Что делает: решение 408 (t193) — нажатие кнопки тревоги «e:<что>.<данные>» (кнопки ставит место события, notify/
//! acts.rs). Данные кнопки — только «что и где»; всё остальное (состояние очереди, ожидание, сделка, отметка настройки)
//! берётся СЕЙЧАС из ядра и шлюза, затем — тот же вопрос «Да, …» / «Отмена» (press_confirm.rs ask) и та же команда
//! ядра Core::command, что у интерфейса:
//! - qh.<площадка>.<кошелёк> — «Снять очередь кошелька» (ReleaseWalletQueue): очередь не остановлена — отказ; nonce
//!   удержанной транзакции ещё не занят — вопрос с предупреждением и «Да, снять при незанятом nonce» (второе
//!   подтверждение, как «Подтверждаю: снять при незанятом nonce» окна WalletQueue.svelte);
//! - wc.<ID ожидания> — «Отменить ожидание» ручного действия (CancelManualWait, решение 405);
//! - od.<ID сделки> — «Открыть сделку»: запись «Ручного управления» сделки (ждёт действия) — её сообщение с кнопками
//!   (в «Управлении» — карточка решения 192); нет записи — карточка сделки (монета, путь, где сейчас), без кнопок;
//! - oe.<биржа>.<монета> — «Открыть вход монеты» (OpenCoinEntry), ub.<монета> — «Убрать из чёрного списка»
//!   (Unblacklist) — исключение из решения 157 (Тигран 18.11), правка настроек — командой ядра с автором «кто нажал»;
//! - oe, ub — notify/press_event_coin.rs; sb, tu, tt, tf — деньги: notify/press_event_money.rs; xd — исполнитель:
//!   notify/press_event_exec.rs.
//!
//! Кто вызывает: notify/press.rs («e»).
//! От чего зависит: press_confirm.rs (ask), press_event_money.rs, core (manual_wait::list, manual_records, шлюз —
//! wallet_queue), mirror_view.rs, ctl/card.rs, alert_card.rs (карточка сделки), settings (coin.*).
//! Настройки: нет (настройки монеты читает press_event_coin.rs).

use crate::core::types::{DexAccountId, ManualAction, ManualRecord, VenueId, WalletQueueState};
use crate::storage::journal::Level;

use super::mirror_state::{Pending, lock};
use super::press::Ctx;
use super::roles::Role;
use super::tg_buttons::{CallbackQuery, CbMessage};

/// Ответ на кнопку с неверными данными.
pub const BAD: &str = "Неизвестная кнопка";

/// Ожидание вопроса «Да, …». Принимает: действие, надпись (None — action.title()), вопрос. Ошибок нет.
pub fn pending(action: ManualAction, title: Option<&str>, question: String) -> Pending {
    Pending {
        key: None,
        title: title.map_or_else(|| action.title().to_string(), str::to_string),
        action,
        question,
    }
}

impl Ctx<'_> {
    /// «e:<что>.<данные>». Принимает: нажатие, сообщение тревоги, объект callback_data. Ошибок нет.
    pub async fn event_button(&self, q: &CallbackQuery, msg: &CbMessage, object: &str) {
        let (op, arg) = object.split_once('.').unwrap_or((object, ""));
        // Ревью t193 (М1): шаги перевода (tf, tx) несут время исходной тревоги — срок считается от неё, а не от шага.
        let Some((sent, arg)) = super::press_event_ttl::alert_time(op, arg, msg.date) else {
            return self.answer(&q.id, Some(BAD)).await;
        };
        // Ответ Тиграна 18.11: кнопки тревог, которые двигают деньги, продают, разворачивают или снимают очередь, —
        // не старше notify.dialog_ttl_s по времени тревоги.
        if super::press_event_ttl::MONEY.contains(&op)
            && let Err(why) = self.fresh_at(sent)
        {
            return self.answer(&q.id, Some(&why)).await;
        }
        let res = match op {
            "qh" => self.ev_queue(arg).await,
            "wc" => ev_wait(self, arg),
            "od" => return self.ev_deal(q, msg, arg).await,
            "oe" => super::press_event_coin::ev_entry(self, arg),
            "ub" => super::press_event_coin::ev_unblock(self, arg),
            _ => return self.money_button(q, msg, op, arg, sent).await,
        };
        self.log(
            Level::Info,
            format!(
                "Telegram: {} нажал кнопку тревоги «{object}»",
                q.from.describe()
            ),
        )
        .await;
        match res {
            Ok(p) => self.ask(&q.id, Some(msg.message_id), p).await,
            Err(why) => self.answer(&q.id, Some(&why)).await,
        }
    }

    /// «Снять очередь кошелька»: состояние очереди сейчас (контракт wallet_queue). Ошибки: текст для нажавшего.
    pub(super) async fn ev_queue(&self, arg: &str) -> Result<Pending, String> {
        let (venue, w) = arg.split_once('.').ok_or(BAD)?;
        let wallet = (w != "-").then(|| w.to_string());
        let acc = wallet.clone().map(DexAccountId);
        let v = self
            .core
            .inner
            .deps
            .gw
            .get_as(&VenueId(venue.to_string()), acc.as_ref())
            .await
            .map_err(|e| e.message)?;
        let q = v.wallet_queue().await.map_err(|e| e.message)?;
        let WalletQueueState::Halted {
            reason,
            nonce,
            hash,
            ..
        } = &q.state
        else {
            return Err("Очередь кошелька не остановлена — снимать нечего".into());
        };
        let who = wallet.as_deref().map_or_else(
            || venue.to_string(),
            |w| match self.settings.accounts().wallet(w) {
                Some(x) => format!("«{}» ({venue})", x.name),
                None => format!("{w} ({venue})"),
            },
        );
        let tx = match (nonce, hash) {
            (Some(n), Some(h)) => format!("nonce {n}, транзакция {h:#x}"),
            (Some(n), None) => format!("nonce {n}"),
            _ => "nonce неизвестен".into(),
        };
        let free = q.nonce_taken == Some(false);
        let action = ManualAction::ReleaseWalletQueue {
            venue: venue.to_string(),
            wallet,
            pending_ok: free,
            // Ревью t193 (Н2): «Да» снимает только ЭТУ остановку (ядро сверяет nonce и hash перед снятием).
            nonce: *nonce,
            hash: (*hash).map(|h| format!("{h:#x}")),
        };
        let (title, question) = if free {
            (
                "Снять очередь при незанятом nonce",
                format!(
                    "Очередь кошелька {who} остановлена: {reason}. Удержано: {tx}.\n⚠️ Nonce ещё не занят в блоке — \
                     висящая транзакция может пройти позже. Всё равно снять очередь?"
                ),
            )
        } else {
            let taken = match q.nonce_taken {
                Some(true) => "nonce уже занят в блоке",
                _ => "занят ли nonce — узел не ответил",
            };
            (
                "Снять очередь кошелька",
                format!(
                    "Очередь кошелька {who} остановлена: {reason}. Удержано: {tx} ({taken}).\nКошелёк и транзакция \
                     проверены вручную? Очередь станет свободной, отправки DEX продолжатся."
                ),
            )
        };
        Ok(pending(action, Some(title), question))
    }

    /// «Открыть сделку»: запись «Ручного управления» сделки с кнопками или карточка сделки. Ошибок нет.
    async fn ev_deal(&self, q: &CallbackQuery, msg: &CbMessage, id: &str) {
        let recs = self.core.manual_records();
        let waiting: Vec<&ManualRecord> = lock(self.state).waiting(&recs);
        let pos = waiting.iter().position(|r| r.deal_id == id);
        let (text, kb) = match pos {
            Some(i) if self.role == Role::Control => {
                let s = super::ctl::card::card(&waiting, i, &waiting[i].key);
                (s.text.clone(), Some(s.markup()))
            }
            Some(i) => super::mirror_view::record_message_for(waiting[i], true),
            None => match super::alert_card::lookup(id) {
                Some(c) => (
                    format!(
                        "Сделка {id}\nМонета: {}\nПуть: {}\nГде сейчас: {}\nЗаписи «Ручного управления» у сделки нет.",
                        c.coin, c.path, c.now
                    ),
                    None,
                ),
                None => (format!("Сделки {id} в учёте нет."), None),
            },
        };
        self.answer(&q.id, None).await;
        if let Err(e) = self
            .tg
            .send_markup(self.chat, &text, kb, Some(msg.message_id))
            .await
        {
            self.log(
                Level::Warn,
                format!("«Открыть сделку {id}» не отправлено: {e}"),
            )
            .await;
        }
    }
}

/// «Отменить ожидание»: ожидание ещё есть (manual_wait::list). Ошибки: текст для нажавшего.
fn ev_wait(c: &Ctx<'_>, id: &str) -> Result<Pending, String> {
    let w = crate::core::manual_wait::list(c.core)
        .into_iter()
        .find(|w| w.id == id)
        .ok_or("Ожидания уже нет — выполнено или отменено")?;
    let q = format!(
        "Отменить ожидание «{}» по {} (нажал: {}, ждёт: {})? Действие не выполнится, новые сделки {} снова начнутся.",
        w.what,
        w.coin,
        w.who,
        w.waits_for.join("; "),
        w.coin
    );
    Ok(pending(
        ManualAction::CancelManualWait { id: id.into() },
        None,
        q,
    ))
}
