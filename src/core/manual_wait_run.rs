//! Файл: core/manual_wait_run.rs
//! Что делает: решение 405 (t191, ответ Тиграна на ревью Н3) — задача ожидания ручного действия (manual_wait.rs):
//! просыпается по `inner.coin_wake` (снята запись идущей сделки — coin_ride::release, освобождён замок монеты —
//! coin_ride::free_active, сохранена сделка — ledger::save, отмена — manual_wait::command) и проверяет монету:
//! - отменено кнопкой — конец: «отменено», пауза снята;
//! - держащая сделка застряла (шаг «застряла»; закрытая / остановленная / ждёт человека — конец, ждём снятия записи) —
//!   конец: отказ с причиной, пауза снята;
//! - идущих сделок нет и замок монеты свободен — ожидание помечается «повтор идёт» (running) и та же команда (тот же
//!   источник и кто) выполняется заново через Core::command; пауза держится, пока повтор не возьмёт монету
//!   (manual_wait::claim в coin_ride::take), иначе снимается после команды (ревью 4, Н3);
//! - иначе — список «чьих окончаний ждём» обновляется на экране, ждём дальше.
//!
//! Конец без выполнения — журнал core.manual_wait (Warn) и событие Command {final, ok: false, text, wait} (Telegram правит
//! сообщение записи: «Не выполнено: причина», кнопки снова доступны — решение 408; экран — строку итога).
//! Кто вызывает: manual_wait.rs (start).
//! От чего зависит: manual_wait.rs, commands.rs (Core::command), manual.rs (publish), view.rs, engine.rs.
//! Настройки: нет.

use serde_json::json;

use crate::core::types::ManualCommand;
use crate::storage::journal::Level;

use super::coin_key::CoinKey;
use super::engine::{Core, lk};
use super::manual_wait as mw;
use super::view::CoreEvent;

/// Итог проверки монеты.
enum Next {
    Cancelled,
    Stuck(String),
    Free,
    Wait(Vec<String>),
}

/// Проверка монеты под ожиданием `id`. Ошибок нет.
fn check(core: &Core, key: &CoinKey, id: &str) -> Next {
    let cancelled = lk(&core.inner.manual_waits)
        .get(key)
        .is_none_or(|w| w.id != id || w.cancel);
    if cancelled {
        return Next::Cancelled;
    }
    let (list, active) = mw::holders(core, key);
    if let Some(why) = mw::stuck_of(&list) {
        return Next::Stuck(why);
    }
    if list.is_empty() && !active {
        return Next::Free;
    }
    Next::Wait(mw::waits_for(&list, active))
}

/// Что делает: запустить задачу ожидания (журнал начала, экран). Принимает: ядро, ключ монеты, ID ожидания, команду,
/// текст начала. Ошибок нет.
pub(super) fn start(core: &Core, key: CoinKey, id: String, cmd: ManualCommand, text: String) {
    let c = core.clone();
    super::manual::publish(core);
    tokio::spawn(async move {
        mw::log(&c, Level::Info, &text).await;
        Box::pin(run(&c, &key, &id, cmd)).await;
    });
}

/// Цикл ожидания (см. шапку). Ошибок нет.
async fn run(core: &Core, key: &CoinKey, id: &str, cmd: ManualCommand) {
    loop {
        // Сначала подписка, потом проверка — пробуждение между ними не теряется (tokio Notify::enable).
        let wake = core.inner.coin_wake.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        match check(core, key, id) {
            Next::Cancelled => {
                return end(
                    core,
                    key,
                    id,
                    &cmd,
                    "ожидание отменено — действие не выполнено, новые сделки этой монеты снова начинаются",
                )
                .await;
            }
            Next::Stuck(why) => {
                let t = format!(
                    "ожидание прервано: {why}. Действие не выполнено, новые сделки этой монеты снова начинаются. Что \
                     сделать: дождитесь, пока бот разберёт эту сделку, или решите её на экране «Ручное управление», \
                     потом нажмите кнопку снова (решение 405)"
                );
                return end(core, key, id, &cmd, &t).await;
            }
            Next::Free => {
                // Ревью 4, Н3: пауза держится, пока повтор не возьмёт монету (manual_wait::claim в take).
                let coin = mw::start_retry(core, key).unwrap_or_default();
                let t = super::manual_wait_text::running(
                    &coin,
                    cmd.action.title(),
                    &super::commands::who(&cmd),
                );
                mw::log(core, Level::Info, &t).await;
                // Та же команда человека — через Core::command (журнал core.command, итог — событие Command).
                // Ревью CI t191 (переполнение стека): будущее команды — в куче.
                let _ = Box::pin(core.command_retry(cmd, id.to_string())).await;
                // Повтор не взял монету (отказ до take) — пауза снимается здесь.
                mw::remove(core, key, id);
                super::manual::publish(core);
                return;
            }
            Next::Wait(list) => {
                let changed = {
                    let mut m = lk(&core.inner.manual_waits);
                    match m.get_mut(key) {
                        Some(w) if w.waits_for != list => {
                            w.waits_for = list;
                            true
                        }
                        _ => false,
                    }
                };
                if changed {
                    super::manual::publish(core);
                }
            }
        }
        wake.await;
    }
}

/// Конец без выполнения: пауза снята, журнал, событие Command (итог кнопки). Ошибок нет.
async fn end(core: &Core, key: &CoinKey, id: &str, cmd: &ManualCommand, why: &str) {
    let coin = mw::remove(core, key, id)
        .map(|w| w.coin)
        .unwrap_or_default();
    super::manual::publish(core);
    let text = format!(
        "«{}» по {coin} (нажал: {}): {why}",
        cmd.action.title(),
        super::commands::who(cmd)
    );
    mw::log(core, Level::Warn, &text).await;
    core.inner.live.send(CoreEvent::Command(json!({
        "final": true, "action": cmd.action, "who": super::commands::who(cmd), "ok": false, "text": text,
        // Решение 408: ID ожидания — Telegram снимает «Ждёт…», пишет «Не выполнено: причина», кнопки снова доступны.
        "wait": id,
    })));
}
