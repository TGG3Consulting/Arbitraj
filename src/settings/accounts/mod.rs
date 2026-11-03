//! Файл: settings/accounts/mod.rs
//! Что делает: кошельки DEX и маппинг «строка → кошелёк» (решение 132, экран «Маппинг кошельков»): записи в памяти и
//! чтение. Кошелёк — «счёт» внутри одной площадки DEX: id (выдаёт система), имя (задаёт Тигран), ИМЯ секрета ключа в
//! зашифрованном файле (сам ключ здесь не хранится никогда — docs/SECURITY.md), адрес своего исполнителя ArbExecutor,
//! ключ очереди транзакций (storage/wallet_queue.rs). Строки маппинга: id биржи (CEX из реестра шлюза), ROW_DEX_DEX
//! (циклы DEX ↔ DEX) и ROW_PROBE (пробы налогов, оценки разрешений и сигналы — только симуляции, без денег).
//! Изменения — edit.rs (история «было/стало» и журнал), загрузка — db.rs, перенос старого кошелька — migrate.rs.
//! Кто вызывает: settings/store.rs (создаёт при загрузке, Store::accounts), ядро (core/dex_account.rs), адаптер DEX
//! (venues/bsc_dex/ctx.rs), api/wallets*.rs.
//! От чего зависит: storage/db.rs, storage/journal.rs.
//! Настройки: нет (это не настройки каталога: свои таблицы и своя история).

pub mod db;
pub mod edit;
pub mod edit_map;
pub mod migrate;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::Serialize;

use crate::storage::db::Db;
use crate::storage::journal::Journal;

/// Строка маппинга циклов DEX ↔ DEX (решение 132).
pub const ROW_DEX_DEX: &str = "dex_dex";
/// Строка маппинга «пробы и сигналы»: налоги, крючки, оценка разрешений, сигналы циклов — симуляции от адреса
/// кошелька без отправки транзакций (решение 132, дополнение — docs/DECISIONS.md).
pub const ROW_PROBE: &str = "probe";

/// Кошелёк DEX. Ключа нет — только имя секрета.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wallet {
    pub id: String,
    pub name: String,
    /// Имя секрета приватного ключа в зашифрованном файле (secrets.enc).
    pub secret: String,
    /// Адрес исполнителя ArbExecutor этого кошелька (0x…); None — не развёрнут.
    pub executor: Option<String>,
    /// Ключ строки очереди транзакций в storage/wallet_queue.rs.
    pub queue_key: String,
    pub created_at: String,
}

/// Всё состояние в памяти.
#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    pub(crate) wallets: BTreeMap<String, Wallet>,
    /// Строка → id кошелька.
    pub(crate) map: BTreeMap<String, String>,
    /// id кошелька, созданного переносом старого ключа (migrate.rs); None — переноса не было.
    pub(crate) legacy: Option<String>,
    /// Перенос выполнен или не нужен (больше не пробовать).
    pub(crate) migrated: bool,
    /// Следующий номер id кошелька (id не переиспользуются: старые сделки и очередь ссылаются на id).
    pub(crate) next: u64,
}

/// Кошельки и маппинг. Дешёво клонируется.
#[derive(Clone)]
pub struct Accounts {
    pub(crate) db: Db,
    pub(crate) journal: Journal,
    pub(crate) st: Arc<RwLock<State>>,
    /// Изменения — строго по одному.
    pub(crate) writer: Arc<tokio::sync::Mutex<()>>,
}

impl Accounts {
    /// Копия состояния под замком чтения. Ошибок нет.
    pub(crate) fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.st.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Все кошельки по id. Ошибок нет.
    pub fn wallets(&self) -> Vec<Wallet> {
        self.read().wallets.values().cloned().collect()
    }

    /// Кошелёк по id. None — нет такого. Ошибок нет.
    pub fn wallet(&self, id: &str) -> Option<Wallet> {
        self.read().wallets.get(id).cloned()
    }

    /// id кошелька строки маппинга (id биржи, ROW_DEX_DEX, ROW_PROBE). None — строки нет. Ошибок нет.
    pub fn row(&self, row: &str) -> Option<String> {
        self.read().map.get(row).cloned()
    }

    /// Все строки маппинга (строка, id кошелька). Ошибок нет.
    pub fn rows(&self) -> Vec<(String, String)> {
        let st = self.read();
        st.map.iter().map(|(r, w)| (r.clone(), w.clone())).collect()
    }

    /// Строки маппинга, к которым привязан кошелёк. Ошибок нет.
    pub fn rows_of(&self, id: &str) -> Vec<String> {
        let st = self.read();
        st.map
            .iter()
            .filter(|(_, w)| w.as_str() == id)
            .map(|(r, _)| r.clone())
            .collect()
    }

    /// Кошелёк переноса (решение 132, пункт 5): им ведутся сделки, начатые до решения (без id счёта). Ошибок нет.
    pub fn legacy(&self) -> Option<String> {
        self.read().legacy.clone()
    }

    /// Предупреждения «деньги смешиваются»: кошелёк привязан больше чем к одной строке (решение Тиграна: можно, но
    /// экран предупреждает). Строка «пробы» денег не двигает и в смешивание не входит.
    /// Возвращает: (кошелёк, его денежные строки). Ошибок нет.
    pub fn shared(&self) -> Vec<(Wallet, Vec<String>)> {
        self.wallets()
            .into_iter()
            .map(|w| {
                let mut rows = self.rows_of(&w.id);
                rows.retain(|r| r != ROW_PROBE);
                (w, rows)
            })
            .filter(|(_, rows)| rows.len() > 1)
            .collect()
    }
}
