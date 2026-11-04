//! Файл: venues/binance/ctx.rs
//! Что делает: общее окружение адаптера Binance — настройки площадки, секреты, HTTP-клиент, журнал, запись сырых
//! сообщений, приёмники окон (решение 125) — и чтение настроек площадки в одном месте.
//! Кто вызывает: все файлы адаптера Binance, main.rs (создаёт).
//! От чего зависит: settings, secrets, storage (journal, raw_log), core/types.
//! Настройки: venue.* с областью «binance» (docs/SETTINGS.md); секреты binance.api_key, binance.api_secret.
//!
//! Официально (https://developers.binance.com/legacy-docs/binance-spot-api-docs/rest-api/request-security): ключ —
//! заголовок X-MBX-APIKEY, подпись HMAC SHA256 секретом ключа (hex). Passphrase у Binance нет. Тип ключа — HMAC
//! (Ed25519 и RSA в документации есть, адаптер их не использует — вопрос Тиграну в отчёте решения 160).

use std::time::Duration;

use zeroize::Zeroizing;

use crate::core::types::VenueId;
use crate::gateway::errors::GatewayError;
use crate::secrets::Vault;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};
use crate::storage::raw_log::RawLog;

/// Идентификатор площадки: по нему ищутся её настройки и секреты.
pub const ID: &str = "binance";
/// Имя секрета API-ключа (тот же список — secrets::KNOWN, для интерфейса).
pub const SECRET_API_KEY: &str = "binance.api_key";
/// Имя секрета секретного ключа (подпись HMAC SHA256).
pub const SECRET_API_SECRET: &str = "binance.api_secret";

/// Окружение адаптера. Дешёво клонируется.
#[derive(Clone)]
pub struct Ctx {
    pub settings: Store,
    pub vault: Vault,
    pub http: reqwest::Client,
    pub journal: Journal,
    pub raw: RawLog,
    /// Записи окон (решение 125): монеты, чьи сырые кадры стакана и сделок пишутся сейчас (gateway/tap.rs).
    pub taps: crate::gateway::tap::Taps,
}

impl Ctx {
    /// Идентификатор площадки в формате ядра. Ошибок нет.
    pub fn venue(&self) -> VenueId {
        VenueId(ID.to_string())
    }

    /// Область настроек площадки. Ошибок нет.
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

    /// Десятичное из настроек площадки. Ошибка NotConfigured — не задано.
    pub fn dec(&self, name: &str) -> Result<rust_decimal::Decimal, GatewayError> {
        self.settings
            .decimal(name, self.at())
            .map_err(|e| self.tag(e))
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

    /// Адреса REST: основной (venue.rest_url) и, если задан, резервный (venue.rest_fallback_url; документация называет
    /// api1…api4.binance.com и api-gcp.binance.com) — резерв только при сбое сети. Ошибка NotConfigured — нет основного.
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

    /// API-ключ и секрет. Ошибки: SecretsLocked, NotConfigured (секрет не введён).
    pub fn keys(&self) -> Result<(Zeroizing<String>, Zeroizing<String>), GatewayError> {
        let key = self.vault.get(SECRET_API_KEY).map_err(|e| self.tag(e))?;
        let secret = self.vault.get(SECRET_API_SECRET).map_err(|e| self.tag(e))?;
        Ok((key, secret))
    }

    /// Символ пары на Binance: «<монета><котируемая>» без разделителя, например CAKEUSDT (exchangeInfo, «symbol»).
    /// Ошибка NotConfigured — не задана venue.quote_asset.
    pub fn symbol(&self, ticker: &str) -> Result<String, GatewayError> {
        Ok(format!("{ticker}{}", self.text("venue.quote_asset")?))
    }

    /// Проставляет в ошибке площадку Binance. Ошибок нет.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }

    /// Запись в журнал от имени Binance. Принимает: уровень, код записи, текст. Ошибок нет (сбой журнала не мешает).
    pub async fn log(&self, level: Level, code: &str, msg: String) {
        let _ = self.journal.write(level, code, Some(ID), msg, None).await;
    }
}
