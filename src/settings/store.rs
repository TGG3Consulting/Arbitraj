//! Файл: settings/store.rs
//! Что делает: хранилище настроек — значения в SQLite + копия в памяти для быстрого чтения,
//! запись изменения в историю и журнал, рассылка изменений на лету.
//! Кто вызывает: api/settings.rs (чтение и запись), все компоненты (чтение через settings/read.rs).
//! От чего зависит: storage/db.rs, storage/journal.rs, settings/catalog.rs, settings/value.rs, settings/live.rs.
//! Настройки: хранит их все. Значений по умолчанию нет: не задано — значит не задано.
//!
//! Изменения выполняются строго по одному (writer): база, кэш и рассылка всегда в одном порядке.
//! Решение 352: поколение значений (generation) растёт на каждое изменение после записи в кэш.
//! Решение 239: копия значений на момент входа в сделку — settings/pinned.rs (тот же Arc значений, без копирования).
//! Решение 409 (t194): значения в памяти — неизменяемый снимок за ArcSwap (settings/snapshot.rs): чтение без замка,
//! изменение — копия значений и новый снимок под замком писателя.
//! Решение 242, дополнение: пара «площадка · тип направления» при записи проверяется по видам площадок шлюза
//! (settings/venue_kinds.rs).
//! История — settings/history.rs. Кошельки DEX и маппинг (решение 132) — settings/accounts/, доступ — accounts(). Решение 128: новый id области «DEX» не длиннее dex.id_max_len (если задана).

use std::sync::Arc;

use chrono::Utc;
use rusqlite::params;
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast};

use crate::storage::db::{Db, DbError};
use crate::storage::journal::{Journal, Level};

use super::catalog;
use super::live::Change;
use super::scope::check_new_dex_id_len;
use super::snapshot::{Snapshot, Values};
use super::value::{Scope, validate};

/// Ошибка записи настройки.
#[derive(Debug, thiserror::Error)]
pub enum SetError {
    #[error("Настройки «{0}» нет в каталоге")]
    Unknown(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Хранилище настроек. Дешёво клонируется.
#[derive(Clone)]
pub struct Store {
    pub(super) db: Db,
    /// Значения в памяти. Решение 409 (t194): неизменяемый снимок за ArcSwap — чтение без замка, запись — новый
    /// снимок (settings/snapshot.rs). Решение 239: копия на момент входа — тот же Arc значений (settings/pinned.rs).
    pub(super) cache: Arc<Snapshot>,
    /// Решение 239: копия на момент входа — только чтение (set отказывает).
    pub(super) pinned: bool,
    pub(super) changes: broadcast::Sender<Change>,
    pub(super) journal: Journal,
    /// Изменения — строго по одному.
    pub(super) writer: Arc<Mutex<()>>,
    /// Кошельки DEX и маппинг «биржа → кошелёк» (решение 132, settings/accounts/) — свои таблицы и история.
    accounts: super::accounts::Accounts,
    /// Решение 211 (ревью В9): тикеры монет по адресу — запасное чтение области «монета» (coin_alias.rs).
    aliases: super::coin_alias::CoinAliases,
    /// Решение 352: поколение значений — растёт на каждое изменение (set), после записи в кэш; снимки (pinned.rs)
    /// делят счётчик с живым хранилищем. Ядро сравнивает поколение, чтобы знать «настройки не менялись». Растёт и на
    /// отметку ядра (store_mark.rs mark_now, решение 355) — она тоже меняет значение.
    pub(super) generation: Arc<std::sync::atomic::AtomicU64>,
    /// Решение 242, дополнение: виды площадок шлюза — проверка пары «площадка · тип направления» при записи
    /// (venue_kinds.rs; сообщает ядро при запуске).
    venue_kinds: super::venue_kinds::VenueKinds,
}

impl Store {
    /// Подписка на изменения настроек (ждать смены значения, например паузы). Ошибок нет.
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// Загружает все заданные значения из базы в память.
    /// Принимает: базу, отправителя изменений (его слушают компоненты и интерфейс), журнал.
    /// Ошибки: база. Повреждённые строки пропускаются с записью в лог процесса.
    pub async fn load(
        db: Db,
        changes: broadcast::Sender<Change>,
        journal: Journal,
    ) -> Result<Self, DbError> {
        let rows: Vec<(String, String, String)> = db
            .call(|c| {
                let mut st = c.prepare("SELECT name, scope, value FROM settings")?;
                let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                it.collect()
            })
            .await?;
        let accounts = super::accounts::Accounts::load(db.clone(), journal.clone()).await?;
        let mut map = Values::default();
        for (name, scope, value) in rows {
            match (
                Scope::from_key(&scope),
                serde_json::from_str::<Value>(&value),
            ) {
                (Some(s), Ok(v)) => map.put(&name, &s, Some(v)),
                _ => tracing::error!(%name, %scope, "пропущена повреждённая строка настроек"),
            }
        }
        Ok(Self {
            db,
            cache: Arc::new(Snapshot::new(Arc::new(map))),
            pinned: false,
            changes,
            journal,
            writer: Arc::new(Mutex::new(())),
            accounts,
            aliases: Default::default(),
            venue_kinds: Default::default(),
            generation: Default::default(),
        })
    }

    /// Решение 352: поколение значений настроек (растёт на каждое изменение). Ошибок нет.
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Кошельки DEX и маппинг (решение 132). Ошибок нет.
    pub fn accounts(&self) -> &super::accounts::Accounts {
        &self.accounts
    }

    /// Ревью 325: то же ли это хранилище (общая копия значений в памяти; снимок решения 239 — другое). Нужно ячейкам
    /// настроек, привязанным к хранилищу (venues/bsc_dex/pools/event_flags.rs). Ошибок нет.
    pub fn same(&self, other: &Store) -> bool {
        Arc::ptr_eq(&self.cache, &other.cache)
    }

    /// Значение ровно в этой области (без подстановки из общей).
    /// Возвращает: значение или None — не задано. Решение 409: снимок без замка (settings/snapshot.rs).
    pub fn get_exact(&self, name: &str, scope: &Scope) -> Option<Value> {
        self.cache.load().get(name, scope).cloned()
    }

    /// Тикеры монет по адресу для запасного чтения области «монета» (решение 211, ревью В9; coin_alias.rs).
    pub fn coin_aliases(&self) -> &super::coin_alias::CoinAliases {
        &self.aliases
    }

    /// Виды площадок шлюза для проверки области «площадка + тип направления» при записи (решение 242, дополнение;
    /// venue_kinds.rs). Ошибок нет.
    pub fn venue_kinds(&self) -> &super::venue_kinds::VenueKinds {
        &self.venue_kinds
    }

    /// Действующее значение: монета сильнее площадки, площадка сильнее всей системы. Решение 211 (второе ревью, п.3):
    /// монета — адрес; значения по адресу нет — из старых значений по тикеру монеты (показной и тикеры площадок,
    /// coin_alias.rs) применяется ТОЛЬКО coin.blocked = да (безопасно: монета не торгуется); любое другое значение по
    /// тикеру неоднозначно и НЕ применяется — дальше площадка и вся система, одна запись в журнал на настройку и тикер.
    /// Принимает: имя, площадку и монету (если применимо). Возвращает: значение или None.
    pub fn resolve(&self, name: &str, venue: Option<&str>, coin: Option<&str>) -> Option<Value> {
        if let Some(id) = venue
            && super::evm_profiles::local_only(name)
            && super::evm_profiles::profile(id).is_some()
        {
            return self.get_exact(name, &Scope::Venue(id.to_string()));
        }
        if let Some(c) = coin {
            if let Some(v) = self.get_exact(name, &Scope::Coin(c.to_string())) {
                return Some(v);
            }
            for t in self.aliases.tickers(c) {
                let Some(v) = self.get_exact(name, &Scope::Coin(t.clone())) else {
                    continue;
                };
                if name == "coin.blocked" && v.as_bool() == Some(true) {
                    self.aliases.report(&self.journal, name, c, &t, true);
                    return Some(v);
                }
                self.aliases.report(&self.journal, name, c, &t, false);
            }
        }
        let order = [
            coin.map(|c| Scope::Coin(c.to_string())),
            venue.map(|v| Scope::Venue(v.to_string())),
            Some(Scope::Global),
        ];
        order
            .into_iter()
            .flatten()
            .find_map(|s| self.get_exact(name, &s))
    }

    /// Все заданные значения — для экрана «Настройки».
    pub fn all(&self) -> Vec<(String, Scope, Value)> {
        let cache = self.cache.load();
        cache
            .iter()
            .map(|(n, s, v)| (n.to_string(), s.clone(), v.clone()))
            .collect()
    }

    /// Задаёт или очищает значение (None — очистить): база + история, кэш, журнал, рассылка.
    /// Если значение не изменилось — ничего не пишет и возвращает None.
    /// Принимает: имя, область, значение, кто меняет.
    /// Ошибки: неизвестное имя (кроме очистки уже сохранённого значения устаревшей настройки), неверное значение
    /// или область, база.
    pub async fn set(
        &self,
        name: &str,
        scope: Scope,
        value: Option<Value>,
        by: &str,
    ) -> Result<Option<Change>, SetError> {
        // Имени нет в каталоге (настройку удалили из кода, например dex.enabled) — разрешено только очистить
        // оставшееся значение: задать его нельзя (решение 59: устаревшие настройки удаляет человек из интерфейса).
        // Решение 239: копия настроек на момент входа — только для чтения.
        if self.pinned {
            return Err(SetError::Invalid(format!(
                "«{name}»: это копия настроек на момент входа в сделку — изменить нельзя"
            )));
        }
        let new = match (catalog::find(name), value) {
            (Some(def), Some(v)) => Some(validate(def, &scope, &v).map_err(SetError::Invalid)?),
            (_, None)
                if catalog::find(name).is_some() || self.get_exact(name, &scope).is_some() =>
            {
                None
            }
            _ => return Err(SetError::Unknown(name.to_string())),
        };
        // Решение 242, дополнение: пара «площадка · тип направления» должна подходить виду площадки шлюза (очистка —
        // без проверки: старую неверную пару можно удалить).
        if let (Scope::VenueDir(id), Some(_)) = (&scope, &new) {
            self.venue_kinds.check(id).map_err(SetError::Invalid)?;
        }
        let _turn = self.writer.lock().await;
        // Решение 128: предел длины нового id DEX (dex.id_max_len, необязательная; очистка не проверяется).
        if let (Scope::Dex(id), Some(_)) = (&scope, &new) {
            let max = self
                .get_exact("dex.id_max_len", &Scope::Global)
                .and_then(|v| v.as_u64());
            let known = self.cache.load().has_scope(&scope);
            check_new_dex_id_len(id, max, known).map_err(SetError::Invalid)?;
        }
        if let Scope::Venue(id) = &scope
            && let Some(p) = super::evm_profiles::profile(id)
            && (new.is_some() || self.venue_kinds.has_dex(id))
        {
            p.check(name, new.as_ref()).map_err(SetError::Invalid)?;
        }
        for p in super::evm_profiles::PROFILES.iter().filter(|p| self.venue_kinds.has_dex(p.venue)) {
            if let Scope::Venue(id) = &scope
                && id != p.venue
                && self.venue_kinds.has_dex(id)
                && p.overlaps(name, new.as_ref())
            {
                return Err(SetError::Invalid(format!("Сеть {} уже зарегистрирована у {}", p.chain, p.venue)));
            }
            let legacy_default = name == "dex.default_chain" && scope == Scope::Global
                || name == "dex.chain" && scope == Scope::Venue("bsc_dex".into());
            if legacy_default && new.as_ref().and_then(Value::as_str) != Some("bsc") {
                return Err(SetError::Invalid("Исторические записи без сети принадлежат bsc".into()));
            }
        }
        let old = self.get_exact(name, &scope);
        if old == new {
            return Ok(None);
        }
        let at = Utc::now();
        let (n, sk, by_s, at_s) = (
            name.to_string(),
            scope.to_key(),
            by.to_string(),
            at.to_rfc3339(),
        );
        let (old_t, new_t) = (
            old.as_ref().map(Value::to_string),
            new.as_ref().map(Value::to_string),
        );
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                match &new_t {
                    Some(v) => tx.execute(
                        "INSERT INTO settings (name, scope, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(name, scope) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                        params![n, sk, v, at_s],
                    )?,
                    None => tx.execute("DELETE FROM settings WHERE name=?1 AND scope=?2", params![n, sk])?,
                };
                tx.execute(
                    "INSERT INTO settings_history (name, scope, old_value, new_value, changed_by, changed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![n, sk, old_t, new_t, by_s, at_s],
                )?;
                tx.commit()
            })
            .await?;
        // Решение 409: новый снимок значений под замком писателя кэша (settings/snapshot.rs); читатели видят старый
        // или новый снимок целиком.
        let applied = self.cache.update(
            |cur| {
                // Решение 355: пока шла запись в базу, отметка ядра (store_mark.rs) могла поменять значение в памяти —
                // она сильнее: значение в памяти не трогаем, отметка сама запишется в базу поверх этого изменения.
                if cur.get(name, &scope) != old.as_ref() {
                    tracing::warn!(
                        name,
                        "настройка изменена в базе, но в памяти осталась отметка ядра (решение 355)"
                    );
                    return (false, None);
                }
                let mut next = cur.clone();
                next.put(name, &scope, new.clone());
                (true, Some(next))
            },
            // Решение 352 (ревью t160 Н4): поколение растёт под замком записи кэша — вместе со значением.
            || {
                self.generation
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            },
        );
        // Ревью слияния (Н7): значение перебито отметкой ядра — в памяти его нет: журнал «изменено» и рассылку не
        // делаем (интерфейс не увидит «изменено» на недействующее значение), вызывающему — понятная причина.
        if !applied {
            return Err(SetError::Invalid(format!(
                "«{name}»: пока изменение записывалось, ядро поставило свою отметку этой настройки (решение 355) — \
                 действует значение отметки, изменение не применено; повторите при необходимости"
            )));
        }
        let change = Change {
            name: name.to_string(),
            scope,
            old,
            new,
            by: by.to_string(),
            at,
        };
        let text = change.describe();
        // Ошибка журнала уже выведена в лог процесса; само изменение сохранено в базе и истории.
        let _ = self
            .journal
            .write(
                Level::Info,
                "settings.changed",
                None,
                text,
                Some(json!(change)),
            )
            .await;
        // Нет слушателей — не ошибка.
        let _ = self.changes.send(change.clone());
        Ok(Some(change))
    }
}
