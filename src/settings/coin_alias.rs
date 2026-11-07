//! Файл: settings/coin_alias.rs
//! Что делает: решение 211 (ревью В9) — запасное чтение настроек области «монета». Монета = адрес: область
//! «coin:0x…». Старые значения по тикеру («coin:CAKE»), которые миграция не перенесла (тикер неоднозначен — несколько
//! адресов, core/coin_scope_migrate.rs), не теряются молча, но и не применяются вслепую (второе ревью 211, п.3):
//! значения по адресу нет — из значений по тикеру монеты (показной тикер и тикеры площадок — их сообщает ядро после
//! сборки справочника, set) применяется ТОЛЬКО coin.blocked = да; любое другое значение по тикеру (и coin.blocked = нет)
//! не применяется — действует площадка/вся система, а в журнал одна запись на настройку и тикер за процесс:
//! «значение по тикеру не применено: неоднозначно, задайте по адресу».
//! Единственное место этой подстановки — Store::resolve (settings/store.rs): все типизированные чтения (read.rs) идут
//! через него.
//! Кто вызывает: settings/store.rs (resolve), core/rebuild.rs (set — после сборки справочника).
//! От чего зависит: settings/scope.rs (is_address_id), storage/journal.rs.
//! Настройки: нет своих (читает любые настройки области «монета»).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use crate::storage::journal::{Journal, Level};

/// Тикеры монет по адресу и уже записанные в журнал подстановки.
#[derive(Clone, Default)]
pub struct CoinAliases {
    /// id области «монета» по адресу («0x…» строчными) → тикеры монеты (показной, затем тикеры площадок).
    map: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// (настройка, адрес, тикер) — уже записано в журнал.
    told: Arc<Mutex<HashSet<(String, String, String)>>>,
}

impl CoinAliases {
    /// Заменяет тикеры монет (после сборки справочника). Принимает: адрес «0x…» → тикеры. Ошибок нет.
    pub fn set(&self, map: HashMap<String, Vec<String>>) {
        *self.map.write().unwrap_or_else(|p| p.into_inner()) = map;
    }

    /// Тикеры монеты по id области «монета». Пусто — id не адрес или монета ядру не известна. Ошибок нет.
    pub fn tickers(&self, coin: &str) -> Vec<String> {
        if !super::scope::is_address_id(coin) {
            return vec![];
        }
        self.map
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(coin)
            .cloned()
            .unwrap_or_default()
    }

    /// Значение по тикеру встречено: первое для этой настройки, монеты и тикера — запись в журнал (задачей, если есть
    /// среда tokio; без неё — только лог процесса). Принимает: журнал, настройку, адрес, тикер, применено ли
    /// (только coin.blocked = да). Ошибок нет.
    pub fn report(&self, journal: &Journal, name: &str, coin: &str, ticker: &str, applied: bool) {
        let fresh = self.told.lock().unwrap_or_else(|p| p.into_inner()).insert((
            name.to_string(),
            coin.to_string(),
            ticker.to_string(),
        ));
        if !fresh {
            return;
        }
        let text = if applied {
            format!(
                "Настройка {name} для монеты {coin} взята по старому тикеру «{ticker}» (блок монеты; решение 211: \
                 монета — адрес) — задайте её по адресу монеты"
            )
        } else {
            format!(
                "Настройка {name} для монеты {coin}: значение по тикеру «{ticker}» не применено: неоднозначно, задайте \
                 по адресу (решение 211)"
            )
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                let j = journal.clone();
                h.spawn(async move {
                    let _ = j
                        .write(
                            Level::Warn,
                            "settings.coin_ticker_fallback",
                            None,
                            text,
                            None,
                        )
                        .await;
                });
            }
            Err(_) => tracing::warn!("{text}"),
        }
    }
}
