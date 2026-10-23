//! Файл: venues/bingx/ctx.rs
//! Что делает: общее окружение адаптера BingX — настройки площадки, секреты, HTTP-клиент,
//! журнал, запись сырых сообщений — и чтение настроек площадки в одном месте.
//! Кто вызывает: все файлы адаптера BingX.
//! От чего зависит: settings, secrets, storage (journal, raw_log), core/types.
//! Настройки: venue.* с областью «bingx» (docs/SETTINGS.md); секреты bingx.api_key, bingx.api_secret.

use std::time::Duration;

use zeroize::Zeroizing;

use crate::core::types::VenueId;
use crate::gateway::errors::GatewayError;
use crate::secrets::Vault;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::Journal;
use crate::storage::raw_log::RawLog;

/// Идентификатор площадки. Это имя самого адаптера, по нему ищутся его настройки и секреты.
pub const ID: &str = "bingx";
/// Имя секрета API-ключа.
pub const SECRET_API_KEY: &str = "bingx.api_key";
/// Имя секрета секретного ключа (для подписи HMAC).
pub const SECRET_API_SECRET: &str = "bingx.api_secret";

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
    /// Идентификатор площадки в формате ядра.
    pub fn venue(&self) -> VenueId {
        VenueId(ID.to_string())
    }

    /// Область настроек площадки.
    pub fn at(&self) -> At<'static> {
        At {
            venue: Some(ID),
            coin: None,
        }
    }

    /// Целое из настроек площадки. Ошибка NotConfigured — не задано.
    pub fn int(&self, name: &str) -> Result<u64, GatewayError> {
        self.settings
            .integer(name, self.at())
            .map_err(|e| self.tag(e))
    }

    /// Текст из настроек площадки. Ошибка NotConfigured — не задано.
    pub fn text(&self, name: &str) -> Result<String, GatewayError> {
        self.settings.text(name, self.at()).map_err(|e| self.tag(e))
    }

    /// Да/нет из настроек площадки. Ошибка NotConfigured — не задано.
    pub fn flag(&self, name: &str) -> Result<bool, GatewayError> {
        self.settings.flag(name, self.at()).map_err(|e| self.tag(e))
    }

    /// Таймаут HTTP из настроек системы. Ошибка NotConfigured — не задано.
    pub fn http_timeout(&self) -> Result<Duration, GatewayError> {
        let ms = self
            .settings
            .integer("http.timeout_ms", At::default())
            .map_err(|e| self.tag(e))?;
        Ok(Duration::from_millis(ms))
    }

    /// Адреса REST: основной и, если задан, резервный (официальная дока: резерв — только при сбое сети).
    /// Ошибка NotConfigured — не задан основной.
    pub fn rest_urls(&self) -> Result<Vec<String>, GatewayError> {
        let mut urls = vec![self.text("venue.rest_url")?];
        if let Ok(fallback) = self.text("venue.rest_fallback_url") {
            urls.push(fallback);
        }
        Ok(urls
            .into_iter()
            .map(|u| u.trim_end_matches('/').to_string())
            .collect())
    }

    /// API-ключ и секрет. Ошибки: SecretsLocked, NotConfigured.
    pub fn keys(&self) -> Result<(Zeroizing<String>, Zeroizing<String>), GatewayError> {
        let key = self.vault.get(SECRET_API_KEY).map_err(|e| self.tag(e))?;
        let secret = self.vault.get(SECRET_API_SECRET).map_err(|e| self.tag(e))?;
        Ok((key, secret))
    }

    /// Символ пары на BingX: «<монета>-<котируемая>», например CAKE-USDT.
    /// Ошибка NotConfigured — не задана venue.quote_asset.
    pub fn symbol(&self, ticker: &str) -> Result<String, GatewayError> {
        Ok(format!("{ticker}-{}", self.text("venue.quote_asset")?))
    }

    /// Проставляет в ошибке площадку BingX.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }
}
