//! Файл: venues/bsc_dex/ctx.rs
//! Что делает: общее окружение адаптера DEX — настройки (площадка, DEX/вид пула, канал), секреты,
//! HTTP-клиент, журнал, запись сырых сообщений — и чтение настроек в одном месте.
//! Кто вызывает: все файлы адаптера DEX.
//! От чего зависит: settings, secrets, storage (journal, raw_log), core/types.
//! Настройки: venue.* и feed.* с областью «bsc_dex», dex.* (область «DEX»), channel.* (область «канал»),
//! bsc.node_ws_url, bsc.node_http_url, http.timeout_ms (docs/SETTINGS.md).
//! Секреты: bsc_dex.wallet_key, bsc.node_ws_key, bsc.node_http_key, channel.<id>.key (docs/SECURITY.md).

use std::time::Duration;

use alloy_primitives::Address;
use rust_decimal::Decimal;
use zeroize::Zeroizing;

use crate::core::types::VenueId;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::secrets::Vault;
use crate::settings::read::At;
use crate::settings::scope::Scope;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};
use crate::storage::raw_log::RawLog;

/// Идентификатор площадки DEX — имя адаптера; по нему ищутся его настройки и секреты.
pub const ID: &str = "bsc_dex";
/// Имя секрета приватного ключа кошелька (тот же список — secrets::KNOWN).
pub const SECRET_WALLET_KEY: &str = "bsc_dex.wallet_key";
/// Метка в адресе узла или канала, на место которой ставится ключ провайдера (docs/SETTINGS.md, «Узел BSC»).
pub const KEY_MARK: &str = "{key}";

/// Окружение адаптера. Дешёво клонируется.
#[derive(Clone)]
pub struct Ctx {
    pub settings: Store,
    pub vault: Vault,
    pub http: reqwest::Client,
    pub journal: Journal,
    pub raw: RawLog,
    /// Общая очередь запросов к узлу HTTP (node/pace.rs).
    pub pace: super::node::pace::Pace,
    /// Резервный узел: удержание на нём после сбоя основного (node/failover.rs).
    pub failover: super::node::failover::Failover,
    /// Справочник пулов от индексатора: очередь запросов и состояние (directory/, решение 58).
    pub index: super::directory::Directory,
}

impl Ctx {
    /// Идентификатор площадки в формате ядра. Ошибок нет.
    pub fn venue(&self) -> VenueId {
        VenueId(ID.to_string())
    }

    /// Область настроек площадки (venue = bsc_dex). Ошибок нет.
    pub fn at(&self) -> At<'static> {
        At {
            venue: Some(ID),
            coin: None,
        }
    }

    /// Целое из настроек площадки. Ошибки: NotConfigured, Internal.
    pub fn int(&self, name: &str) -> Result<u64, GatewayError> {
        self.settings
            .integer(name, self.at())
            .map_err(|e| self.tag(e))
    }

    /// Десятичное число из настроек площадки. Ошибки: NotConfigured, Internal.
    pub fn dec(&self, name: &str) -> Result<Decimal, GatewayError> {
        self.settings
            .decimal(name, self.at())
            .map_err(|e| self.tag(e))
    }

    /// Текст (или вариант выбора) из настроек площадки. Ошибки: NotConfigured, Internal.
    pub fn text(&self, name: &str) -> Result<String, GatewayError> {
        self.settings.text(name, self.at()).map_err(|e| self.tag(e))
    }

    /// Да/нет из настроек площадки. Ошибки: NotConfigured, Internal.
    pub fn flag(&self, name: &str) -> Result<bool, GatewayError> {
        self.settings.flag(name, self.at()).map_err(|e| self.tag(e))
    }

    /// Адрес контракта из настроек площадки. Ошибки: NotConfigured, Internal.
    pub fn addr(&self, name: &str) -> Result<Address, GatewayError> {
        self.settings
            .address(name, self.at())
            .map_err(|e| self.tag(e))
    }

    /// Настройка области «DEX» (id — имя DEX или вид пула). Ошибки: NotConfigured, Internal.
    pub fn dex_addr(&self, name: &str, id: &str) -> Result<Address, GatewayError> {
        let scope = Scope::Dex(id.to_string());
        self.settings
            .address_in(name, &scope)
            .map_err(|e| self.tag(e))
    }

    /// Да/нет в области «DEX». Ошибки: NotConfigured, Internal.
    pub fn dex_flag(&self, name: &str, id: &str) -> Result<bool, GatewayError> {
        let scope = Scope::Dex(id.to_string());
        self.settings.flag_in(name, &scope).map_err(|e| self.tag(e))
    }

    /// Целое в области «DEX». Ошибки: NotConfigured, Internal.
    pub fn dex_int(&self, name: &str, id: &str) -> Result<u64, GatewayError> {
        let scope = Scope::Dex(id.to_string());
        self.settings
            .integer_in(name, &scope)
            .map_err(|e| self.tag(e))
    }

    /// Десятичное число в области «DEX». Ошибки: NotConfigured, Internal.
    pub fn dex_dec(&self, name: &str, id: &str) -> Result<Decimal, GatewayError> {
        let scope = Scope::Dex(id.to_string());
        self.settings
            .decimal_in(name, &scope)
            .map_err(|e| self.tag(e))
    }

    /// Текст в области «DEX». Ошибки: NotConfigured, Internal.
    pub fn dex_text(&self, name: &str, id: &str) -> Result<String, GatewayError> {
        let scope = Scope::Dex(id.to_string());
        self.settings.text_in(name, &scope).map_err(|e| self.tag(e))
    }

    /// Список строк в области «DEX» (вид TextList). Ошибки: NotConfigured (не задан), Internal (не список строк).
    pub fn dex_list(&self, name: &str, id: &str) -> Result<Vec<String>, GatewayError> {
        let v = self.settings.get_exact(name, &Scope::Dex(id.to_string()));
        let v = v.ok_or_else(|| {
            let m = format!("Не задана настройка {name} [dex:{id}] — задайте её в интерфейсе");
            self.err(ErrorCode::NotConfigured, m)
        })?;
        let list = v.as_array().and_then(|a| {
            a.iter()
                .map(|x| x.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        });
        list.ok_or_else(|| self.err(ErrorCode::Internal, format!("{name}: хранится не списком")))
    }

    /// Целое из настроек всей системы (например http.timeout_ms). Ошибки: NotConfigured, Internal.
    pub fn global_int(&self, name: &str) -> Result<u64, GatewayError> {
        self.settings
            .integer(name, At::default())
            .map_err(|e| self.tag(e))
    }

    /// Таймаут ответа внешнего сервиса (http.timeout_ms): HTTP узла и ответы узла по WebSocket.
    /// Ошибки: NotConfigured, Internal.
    pub fn timeout(&self) -> Result<Duration, GatewayError> {
        Ok(Duration::from_millis(self.global_int("http.timeout_ms")?))
    }

    /// Адрес узла из настройки с подстановкой ключа провайдера вместо {key} (ключ — секрет).
    /// Принимает: имя настройки адреса, имя секрета ключа. Возвращает: готовый адрес (в журнал его не писать).
    /// Ошибки: NotConfigured (адрес или ключ не задан), SecretsLocked.
    pub fn node_url(&self, setting: &str, secret: &str) -> Result<Zeroizing<String>, GatewayError> {
        let url = self
            .settings
            .text(setting, At::default())
            .map_err(|e| self.tag(e))?;
        self.with_key(&url, secret)
    }

    /// Подставляет ключ из секрета вместо {key}; нет метки — адрес как есть, секрет не нужен.
    /// Ошибки: SecretsLocked, NotConfigured (секрет не введён).
    pub fn with_key(&self, url: &str, secret: &str) -> Result<Zeroizing<String>, GatewayError> {
        if !url.contains(KEY_MARK) {
            return Ok(Zeroizing::new(url.to_string()));
        }
        let key = self.vault.get(secret).map_err(|e| self.tag(e))?;
        Ok(Zeroizing::new(url.replace(KEY_MARK, key.as_str())))
    }

    /// Проставляет в ошибке площадку bsc_dex. Ошибок нет.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }

    /// Ошибка с площадкой bsc_dex. Принимает: код, текст. Возвращает: GatewayError.
    pub fn err(&self, code: ErrorCode, message: impl Into<String>) -> GatewayError {
        GatewayError::new(code, Some(self.venue()), message)
    }

    /// Запись в журнал с площадкой bsc_dex (ошибка базы уже выведена журналом в лог процесса).
    /// Принимает: уровень, вид записи, текст, данные. Ошибок не возвращает.
    pub async fn log(
        &self,
        level: Level,
        kind: &str,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) {
        let _ = self
            .journal
            .write(level, kind, Some(ID), message, data)
            .await;
    }
}
