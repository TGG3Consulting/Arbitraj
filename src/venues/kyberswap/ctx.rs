//! Файл: venues/kyberswap/ctx.rs
//! Что делает: общее окружение адаптера DEX через агрегатор KyberSwap — настройки (площадка и kyber.*), секреты,
//! HTTP-клиент, журнал, запись сырых сообщений — и чтение настроек в одном месте. Адрес узла BSC для исполнения с
//! подстановкой ключа провайдера вместо {key} — тоже здесь.
//! Кто вызывает: все файлы адаптера kyberswap.
//! От чего зависит: settings, secrets, storage (journal, raw_log), core/types, gateway/http, gateway/errors.
//! Настройки: venue.* [kyberswap] (quote_asset, bsc_network, enabled), kyber.* (все), http.* (docs/SETTINGS.md).
//! Секреты: bsc_dex.wallet_key (один кошелёк, решение 45), kyber.node_http_key (docs/SECURITY.md).

use std::time::Duration;

use alloy_primitives::Address;
use rust_decimal::Decimal;
use zeroize::Zeroizing;

use crate::core::types::VenueId;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::http::Limits;
use crate::secrets::Vault;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};
use crate::storage::raw_log::RawLog;

/// Идентификатор площадки — имя адаптера; по нему ищутся его настройки venue.* и секреты.
pub const ID: &str = "kyberswap";
/// Имя секрета приватного ключа кошелька — тот же, что у bsc_dex (один кошелёк, решение 45).
pub const SECRET_WALLET_KEY: &str = "bsc_dex.wallet_key";
/// Имя секрета ключа провайдера узла BSC для исполнения.
pub const SECRET_NODE_KEY: &str = "kyber.node_http_key";
/// Метка в адресе узла, на место которой ставится ключ провайдера (в журнал адрес не пишется).
pub const KEY_MARK: &str = "{key}";

/// Окружение адаптера. Дешёво клонируется.
#[derive(Clone)]
pub struct Ctx {
    pub settings: Store,
    pub vault: Vault,
    pub http: reqwest::Client,
    pub journal: Journal,
    pub raw: RawLog,
}

impl Ctx {
    /// Идентификатор площадки в формате ядра. Ошибок нет.
    pub fn venue(&self) -> VenueId {
        VenueId(ID.to_string())
    }

    /// Область настроек площадки (venue = kyberswap). Ошибок нет.
    pub fn at(&self) -> At<'static> {
        At {
            venue: Some(ID),
            coin: None,
        }
    }

    /// Текст настройки площадки (venue.*). Ошибки: NotConfigured, Internal.
    pub fn venue_text(&self, name: &str) -> Result<String, GatewayError> {
        self.settings.text(name, self.at()).map_err(|e| self.tag(e))
    }

    /// Да/нет настройки площадки (venue.*). Ошибки: NotConfigured, Internal.
    pub fn venue_flag(&self, name: &str) -> Result<bool, GatewayError> {
        self.settings.flag(name, self.at()).map_err(|e| self.tag(e))
    }

    /// Текст глобальной настройки (kyber.*, http.*). Ошибки: NotConfigured, Internal.
    pub fn text(&self, name: &str) -> Result<String, GatewayError> {
        self.settings
            .text(name, At::default())
            .map_err(|e| self.tag(e))
    }

    /// Целое глобальной настройки. Ошибки: NotConfigured, Internal.
    pub fn int(&self, name: &str) -> Result<u64, GatewayError> {
        self.settings
            .integer(name, At::default())
            .map_err(|e| self.tag(e))
    }

    /// Десятичное число глобальной настройки. Ошибки: NotConfigured, Internal.
    pub fn dec(&self, name: &str) -> Result<Decimal, GatewayError> {
        self.settings
            .decimal(name, At::default())
            .map_err(|e| self.tag(e))
    }

    /// Да/нет глобальной настройки. Ошибки: NotConfigured, Internal.
    pub fn flag(&self, name: &str) -> Result<bool, GatewayError> {
        self.settings
            .flag(name, At::default())
            .map_err(|e| self.tag(e))
    }

    /// Адрес контракта глобальной настройки. Ошибки: NotConfigured, Internal.
    pub fn addr(&self, name: &str) -> Result<Address, GatewayError> {
        self.settings
            .address(name, At::default())
            .map_err(|e| self.tag(e))
    }

    /// Сроки и пределы HTTP-обмена (gateway/http.rs): заголовки — kyber.http_timeout_ms, тело — http.body_idle_ms /
    /// http.total_ms / http.max_body_bytes. Общие для API агрегатора и узла BSC. Ошибки: NotConfigured, Internal.
    pub fn limits(&self) -> Result<Limits, GatewayError> {
        let ms = |n: &str| self.int(n).map(Duration::from_millis);
        Ok(Limits {
            head: ms("kyber.http_timeout_ms")?,
            idle: ms("http.body_idle_ms")?,
            total: ms("http.total_ms")?,
            max_bytes: self.int("http.max_body_bytes")? as usize,
        })
    }

    /// Адрес узла BSC для исполнения (kyber.node_http_url) с подстановкой ключа вместо {key} (ключ — секрет, в журнал
    /// не пишется). Ошибки: NotConfigured (адрес или ключ не задан), SecretsLocked.
    pub fn node_url(&self) -> Result<Zeroizing<String>, GatewayError> {
        let url = self.text("kyber.node_http_url")?;
        if !url.contains(KEY_MARK) {
            return Ok(Zeroizing::new(url));
        }
        let key = self.vault.get(SECRET_NODE_KEY).map_err(|e| self.tag(e))?;
        Ok(Zeroizing::new(url.replace(KEY_MARK, key.as_str())))
    }

    /// Базовый адрес запроса агрегатора: kyber.base_url + «/» + kyber.chain + «/api/v1». Хвост добавляет вызывающий
    /// (rest.rs). Ошибки: NotConfigured (адрес или сеть не заданы).
    pub fn api_base(&self) -> Result<String, GatewayError> {
        let base = self.text("kyber.base_url")?;
        let chain = self.text("kyber.chain")?;
        Ok(format!(
            "{}/{}/api/v1",
            base.trim_end_matches('/'),
            chain.trim_matches('/')
        ))
    }

    /// Ошибка «настройка не задана» с её именем (GatewayError.setting). Ошибок нет.
    pub fn not_set(&self, name: &str, message: impl Into<String>) -> GatewayError {
        self.tag(GatewayError::not_set(None, name, message))
    }

    /// Проставляет в ошибке площадку kyberswap. Ошибок нет.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }

    /// Ошибка с площадкой kyberswap. Принимает: код, текст. Возвращает: GatewayError.
    pub fn err(&self, code: ErrorCode, message: impl Into<String>) -> GatewayError {
        GatewayError::new(code, Some(self.venue()), message)
    }

    /// Запись в журнал с площадкой kyberswap. Ошибок не возвращает.
    pub async fn log(&self, level: Level, kind: &str, message: impl Into<String>) {
        let _ = self
            .journal
            .write(level, kind, Some(ID), message, None)
            .await;
    }
}
