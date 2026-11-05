//! Файл: venues/coinex/ctx.rs
//! Что делает: общее окружение адаптера CoinEx — настройки площадки, секреты (access_id и secret_key), HTTP-клиент,
//! журнал, запись сырых сообщений — и чтение настроек площадки в одном месте.
//! Кто вызывает: все файлы адаптера CoinEx, main.rs (создаёт).
//! От чего зависит: settings, secrets, storage (journal, raw_log), core/types.
//! Настройки: venue.* с областью «coinex» (docs/SETTINGS.md); секреты coinex.api_key, coinex.api_secret
//! (docs/SECURITY.md).
//!
//! Официально (Authentication, https://docs.coinex.com/api/v2/authorization): ключ — пара access_id и secret_key со
//! страницы API management; «Please do not directly transmit the secret_key» — секрет уходит только подписью. Passphrase
//! у CoinEx нет.

use std::time::Duration;

use zeroize::Zeroizing;

use crate::core::types::VenueId;
use crate::gateway::errors::GatewayError;
use crate::secrets::Vault;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::Journal;
use crate::storage::raw_log::RawLog;

/// Идентификатор площадки: по нему ищутся её настройки и секреты.
pub const ID: &str = "coinex";
/// Имя секрета access_id (заголовок X-COINEX-KEY; тот же список — secrets::KNOWN, для интерфейса).
pub const SECRET_API_KEY: &str = "coinex.api_key";
/// Имя секрета secret_key (подпись HMAC-SHA256).
pub const SECRET_API_SECRET: &str = "coinex.api_secret";

/// Ключ и секрет — только в памяти, затираются при удалении.
pub struct Keys {
    pub key: Zeroizing<String>,
    pub secret: Zeroizing<String>,
}

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

    /// Сроки и пределы HTTP-обмена (gateway/http.rs): http.timeout_ms, http.body_idle_ms, http.total_ms,
    /// http.max_body_bytes. Ошибки: NotConfigured (любая не задана).
    pub fn limits(&self) -> Result<crate::gateway::http::Limits, GatewayError> {
        let int = |n: &str| {
            self.settings
                .integer(n, At::default())
                .map_err(|e| self.tag(e))
        };
        Ok(crate::gateway::http::Limits {
            head: Duration::from_millis(int("http.timeout_ms")?),
            idle: Duration::from_millis(int("http.body_idle_ms")?),
            total: Duration::from_millis(int("http.total_ms")?),
            max_bytes: int("http.max_body_bytes")? as usize,
        })
    }

    /// Таймаут HTTP из настроек системы (время на подключение WebSocket). Ошибка NotConfigured — не задано.
    pub fn http_timeout(&self) -> Result<Duration, GatewayError> {
        let ms = self
            .settings
            .integer("http.timeout_ms", At::default())
            .map_err(|e| self.tag(e))?;
        Ok(Duration::from_millis(ms))
    }

    /// Адреса REST: основной (venue.rest_url) и, если задан, резервный (venue.rest_fallback_url) — резерв только при
    /// сбое сети. Адрес — без «/v2»: путь версии входит в подпись (rest.rs). Ошибка NotConfigured — не задан основной.
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

    /// Ключ и секрет. Ошибки: SecretsLocked, NotConfigured (секрет не введён).
    pub fn keys(&self) -> Result<Keys, GatewayError> {
        Ok(Keys {
            key: self.vault.get(SECRET_API_KEY).map_err(|e| self.tag(e))?,
            secret: self.vault.get(SECRET_API_SECRET).map_err(|e| self.tag(e))?,
        })
    }

    /// Проставляет в ошибке площадку CoinEx.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }

    /// Запись в журнал от имени CoinEx. Принимает: уровень, код записи, текст. Ошибок нет (сбой журнала не мешает).
    pub async fn log(&self, level: crate::storage::journal::Level, code: &str, msg: String) {
        let _ = self.journal.write(level, code, Some(ID), msg, None).await;
    }
}
