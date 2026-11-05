//! Файл: venues/bitget/ctx.rs
//! Что делает: общее окружение адаптера Bitget — настройки площадки, секреты (ключ, секрет, passphrase), HTTP-клиент,
//! журнал, запись сырых сообщений — и чтение настроек площадки в одном месте.
//! Кто вызывает: все файлы адаптера Bitget, main.rs (создаёт).
//! От чего зависит: settings, secrets, storage (journal, raw_log), core/types.
//! Настройки: venue.* с областью «bitget» (docs/SETTINGS.md); секреты bitget.api_key, bitget.api_secret,
//! bitget.api_passphrase (docs/SECURITY.md).
//!
//! Официально (Classic → REST API, https://www.bitget.com/docs/classic/rest-api): приватный запрос несёт ACCESS-KEY,
//! ACCESS-SIGN, ACCESS-TIMESTAMP, ACCESS-PASSPHRASE («The passphrase you specified when creating the API key»).

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
pub const ID: &str = "bitget";
/// Имя секрета API-ключа (тот же список — secrets::KNOWN, для интерфейса).
pub const SECRET_API_KEY: &str = "bitget.api_key";
/// Имя секрета секретного ключа (подпись HMAC SHA256).
pub const SECRET_API_SECRET: &str = "bitget.api_secret";
/// Имя секрета passphrase ключа (задаётся при создании ключа на Bitget).
pub const SECRET_API_PASSPHRASE: &str = "bitget.api_passphrase";

/// Ключ, секрет и passphrase — только в памяти, затираются при удалении.
pub struct Keys {
    pub key: Zeroizing<String>,
    pub secret: Zeroizing<String>,
    pub passphrase: Zeroizing<String>,
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
    /// сбое сети. Ошибка NotConfigured — не задан основной.
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

    /// Ключ, секрет и passphrase. Ошибки: SecretsLocked, NotConfigured (секрет не введён).
    pub fn keys(&self) -> Result<Keys, GatewayError> {
        Ok(Keys {
            key: self.vault.get(SECRET_API_KEY).map_err(|e| self.tag(e))?,
            secret: self.vault.get(SECRET_API_SECRET).map_err(|e| self.tag(e))?,
            passphrase: self
                .vault
                .get(SECRET_API_PASSPHRASE)
                .map_err(|e| self.tag(e))?,
        })
    }

    /// Проставляет в ошибке площадку Bitget.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }

    /// Запись в журнал от имени Bitget. Принимает: уровень, код записи, текст. Ошибок нет (сбой журнала не мешает).
    pub async fn log(&self, level: crate::storage::journal::Level, code: &str, msg: String) {
        let _ = self.journal.write(level, code, Some(ID), msg, None).await;
    }
}

impl Ctx {
    /// Решение 178: HTTP-клиент REST площадки — с исходящего адреса venue.local_ip, если он задан
    /// (gateway/http_bind.rs); иначе общий. Ошибки: NotConfigured — не IP-адрес; Internal — клиент не собран.
    pub fn client(&self) -> Result<reqwest::Client, crate::gateway::errors::GatewayError> {
        crate::gateway::http_bind::client(&self.http, &self.settings, ID).map_err(|e| self.tag(e))
    }
}
