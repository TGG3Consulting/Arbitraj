//! Файл: notify/ctl/progress.rs
//! Что делает: «Да, перевести USDT» (решение 176) — команда ядра TransferUsdt отдельной задачей (перевод идёт минуты,
//! опрос Telegram его не ждёт) и ход перевода правкой ТОГО ЖЕ сообщения вопроса: «проверка → отправлено → в сети
//! (транзакция) → итог» (итог — текст ядра: «зачислено …» или причина отказа, notify/human тексты перегона уходят
//! событиями usdt.relayed / usdt.relay_failed как раньше). Ход читается из памяти ядра (Core::usdt_move), не у биржи;
//! правка — не чаще notify.live_refresh_s и только если ход изменился (не задана — только итог). Ответ 429 на итог —
//! одна повторная правка ровно через parameters.retry_after.
//! Кто вызывает: notify/press_confirm.rs (confirm).
//! От чего зависит: press_confirm.rs (run_pending — общий путь команды), core (usdt_move), tg_methods.rs, settings.
//! Настройки: notify.live_refresh_s.

use std::time::Duration;

use crate::core::usdt_transfer::Stage;
use crate::settings::read::At;

use super::super::mirror_state::Pending;
use super::super::press::Ctx;
use super::super::press_confirm::run_pending;
use super::super::tg_buttons::User;
use super::super::tg_methods::retry_after;

/// Ход перевода словами. Принимает: шаг. Ошибок нет.
pub fn stage_text(s: &Stage) -> String {
    match s {
        Stage::Checking => "проверка и ожидание вывода…".into(),
        Stage::Sent => "отправлено — биржа выводит, ждём транзакцию в сети…".into(),
        Stage::OnChain { tx } => format!(
            "в сети (транзакция {}) — ждём зачисления…",
            super::super::human::addr(tx)
        ),
    }
}

/// Запускает перевод с ходом в сообщении вопроса. Принимает: контекст нажатия, ожидание, кто нажал,
/// (чат, сообщение вопроса), текст вопроса, ключ отправителя (End::key). Ошибок нет: итог — в сообщении и журнале.
pub async fn spawn(
    c: &Ctx<'_>,
    p: Pending,
    from: User,
    (chat, msg): (i64, i64),
    base: String,
    key: String,
) {
    let (tg, core, journal, state) = (
        c.tg.clone(),
        c.core.clone(),
        c.journal.clone(),
        c.state.clone(),
    );
    let every = c
        .settings
        .integer("notify.live_refresh_s", At::default())
        .ok()
        .map(Duration::from_secs);
    let chat_s = chat.to_string();
    let first = format!("{base}\n\nХод: {}", stage_text(&Stage::Checking));
    let _ = tg.edit_text_markup(&chat_s, msg, &first, None).await;
    tokio::spawn(async move {
        let run = run_pending(&core, &journal, &state, &p, &from, chat);
        tokio::pin!(run);
        let mut last = Stage::Checking;
        let text = loop {
            tokio::select! {
                t = &mut run => break t,
                _ = tick(every) => {
                    let Some(m) = core.usdt_move(&key) else { continue };
                    if m.stage != last {
                        let t = format!("{base}\n\nХод: {}", stage_text(&m.stage));
                        if tg.edit_text_markup(&chat_s, msg, &t, None).await.is_ok() {
                            last = m.stage;
                        }
                    }
                }
            }
        };
        let done = format!("{base}\n\n{text}");
        if let Err(e) = tg.edit_text_markup(&chat_s, msg, &done, None).await {
            match retry_after(&e) {
                Some(s) => {
                    tokio::time::sleep(Duration::from_secs(s)).await;
                    let _ = tg.edit_text_markup(&chat_s, msg, &done, None).await;
                }
                None => {
                    let t = format!("Перевод USDT: итог в Telegram не показан: {e}");
                    let _ = journal
                        .write(
                            crate::storage::journal::Level::Warn,
                            "notify",
                            None,
                            t,
                            None,
                        )
                        .await;
                }
            }
        }
    });
}

/// Пауза до следующей проверки хода (None — никогда: только итог). Ошибок нет.
async fn tick(every: Option<Duration>) {
    match every {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}
