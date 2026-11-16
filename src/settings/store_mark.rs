//! Файл: settings/store_mark.rs
//! Что делает: решение 355 (задача t158, ревью Б1) — запрещающая отметка ядра (монета в чёрном списке, входы с биржи
//! закрыты) действует МГНОВЕННО: значение ставится в память хранилища настроек синхронно — его сразу видят все, кто
//! читает настройку (кандидаты, предпроверка, исполнитель: blacklist.rs, withdraw_block::check — те же точки, что и
//! раньше), и рассылается как изменение; в базу (settings + история) и в журнал оно пишется своей задачей — сделка
//! не ждёт основную базу. Новое значение считается под замком памяти из текущего (функция f) — чтение и запись одного
//! ключа без гонки (например, список бирж монеты). Запись в базу не удалась — запись журнала Error, отметка в памяти
//! остаётся; повтор через settings.mark_retry_ms (не задана — без повтора: в журнале «внесите вручную», после
//! перезапуска отметки не будет). Пока запись в базу ждёт, человек (или другой код) поменял ту же настройку — побеждает
//! более позднее значение в памяти: фоновая запись видит, что значение уже другое, и не пишет устаревшее.
//! Кто вызывает: core/sell_slip.rs, core/credit_shortfall.rs, core/withdraw_block.rs, core/exit_gap.rs.
//! От чего зависит: settings/store.rs (поля Store), settings/catalog, settings/value.rs (validate), storage/db.rs,
//! storage/journal.rs, tokio.
//! Настройки: settings.mark_retry_ms (необязательная).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rusqlite::params;
use serde_json::{Value, json};

use crate::storage::db::DbError;
use crate::storage::journal::Level;

use super::catalog;
use super::live::Change;
use super::read::At;
use super::store::{SetError, Store};
use super::value::{Scope, validate};

/// Пауза перед повтором записи отметки в базу.
pub const RETRY: &str = "settings.mark_retry_ms";

impl Store {
    /// Что делает: отметка ядра — новое значение в памяти сейчас (из текущего: f), в базу и журнал — своей задачей.
    /// Принимает: имя, область (только «вся система», «площадка», «монета»), функцию «текущее → новое» (None —
    /// очистить), кто меняет. Возвращает: изменение (None — значение не изменилось). Ошибки: копия на момент входа,
    /// неизвестное имя, неверное значение или область. Не ждёт базу и не await.
    pub fn mark_now(
        &self,
        name: &str,
        scope: Scope,
        f: impl FnOnce(Option<&Value>) -> Option<Value>,
        by: &str,
    ) -> Result<Option<Change>, SetError> {
        if self.pinned {
            return Err(SetError::Invalid(format!(
                "«{name}»: копия настроек на момент входа — изменить нельзя"
            )));
        }
        let def = catalog::find(name).ok_or_else(|| SetError::Unknown(name.to_string()))?;
        if !matches!(scope, Scope::Global | Scope::Venue(_) | Scope::Coin(_)) {
            return Err(SetError::Invalid(format!(
                "«{name}»: отметка ядра — только области «вся система», «площадка», «монета»"
            )));
        }
        let key = (name.to_string(), scope.clone());
        let (old, new) = {
            let mut guard = self.cache.write().unwrap_or_else(|p| p.into_inner());
            let old = guard.get(&key).cloned();
            let new = match f(old.as_ref()) {
                Some(v) => Some(validate(def, &scope, &v).map_err(SetError::Invalid)?),
                None => None,
            };
            if old == new {
                return Ok(None);
            }
            // Решение 239: снимки (pinned.rs) держат прежний Arc — копия значений только если снимок жив.
            let cache = Arc::make_mut(&mut *guard);
            match &new {
                Some(v) => cache.insert(key, v.clone()),
                None => cache.remove(&key),
            };
            (old, new)
        };
        let change = Change {
            name: name.to_string(),
            scope,
            old,
            new,
            by: by.to_string(),
            at: Utc::now(),
        };
        // Нет слушателей — не ошибка.
        let _ = self.changes.send(change.clone());
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(persist(self.clone(), change.clone()));
            }
            Err(_) => tracing::error!(name, "отметка ядра не записана в базу: нет рантайма tokio"),
        }
        Ok(Some(change))
    }
}

/// Записывает отметку в базу и журнал (по одной с прочими изменениями — замок writer). Значение в памяти уже другое
/// (изменено позже) — не пишет. Ошибка — журнал Error и повтор через settings.mark_retry_ms (не задана — без
/// повтора). Принимает: хранилище, изменение. Ошибок нет.
async fn persist(s: Store, ch: Change) {
    loop {
        let res = {
            let _turn = s.writer.lock().await;
            if s.get_exact(&ch.name, &ch.scope) != ch.new {
                return;
            }
            write(&s, &ch).await
        };
        let err = match res {
            Ok(()) => {
                let _ = s
                    .journal
                    .write(
                        Level::Info,
                        "settings.changed",
                        None,
                        ch.describe(),
                        Some(json!(ch)),
                    )
                    .await;
                return;
            }
            Err(e) => e,
        };
        let retry = s.integer(RETRY, At::default()).ok();
        let tail = match retry {
            Some(ms) => format!("повтор через {ms} мс"),
            None => format!(
                "повтора нет ({RETRY} не задана) — внесите вручную: после перезапуска отметки не будет"
            ),
        };
        let msg = format!(
            "Отметка «{}» действует в памяти, но в базу не записана: {err}; {tail}",
            ch.describe()
        );
        let _ = s
            .journal
            .write(
                Level::Error,
                "settings.mark_unsaved",
                None,
                msg,
                Some(json!(ch)),
            )
            .await;
        match retry {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            None => return,
        }
    }
}

/// Значение и история — одной транзакцией (как Store::set). Принимает: хранилище, изменение. Ошибки: база.
async fn write(s: &Store, ch: &Change) -> Result<(), DbError> {
    let (n, sk, by, at) = (
        ch.name.clone(),
        ch.scope.to_key(),
        ch.by.clone(),
        ch.at.to_rfc3339(),
    );
    let (old_t, new_t) = (
        ch.old.as_ref().map(Value::to_string),
        ch.new.as_ref().map(Value::to_string),
    );
    s.db.call(move |c| {
        let tx = c.transaction()?;
        match &new_t {
            Some(v) => tx.execute(
                "INSERT INTO settings (name, scope, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(name, scope) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                params![n, sk, v, at],
            )?,
            None => tx.execute(
                "DELETE FROM settings WHERE name=?1 AND scope=?2",
                params![n, sk],
            )?,
        };
        tx.execute(
            "INSERT INTO settings_history (name, scope, old_value, new_value, changed_by, changed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![n, sk, old_t, new_t, by, at],
        )?;
        tx.commit()
    })
    .await
}
