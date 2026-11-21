//! Файл: venues/gate/ctx.rs
//! Что делает: общее окружение адаптера Gate — настройки площадки, секреты, HTTP-клиент, журнал, запись сырых
//! сообщений, приёмники окон (решение 125) — и чтение настроек площадки в одном месте.
//! Кто вызывает: все файлы адаптера Gate.
//! От чего зависит: settings, secrets, storage (journal, raw_log), core/types.
//! Настройки: venue.* с областью «gate» (docs/SETTINGS.md); секреты gate.api_key, gate.api_secret.
//!
//! Символ пары — по официальной документации: «Currency pair» в виде `BTC_USDT` («For `BTC_USDT`, the base currency is
//! `BTC` and the quote currency is `USDT`» — описание поля amount модели Order, API v4 → Spot → Create an order,
//! https://www.gate.com/docs/developers/apiv4/en/ ; SDK https://github.com/gate/gateapi-python/blob/master/docs/Order.md).

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
pub const ID: &str = "gate";
/// Имя секрета API-ключа (тот же список — secrets::KNOWN, для интерфейса).
pub const SECRET_API_KEY: &str = "gate.api_key";
/// Имя секрета секретного ключа (подпись HMAC-SHA512).
pub const SECRET_API_SECRET: &str = "gate.api_secret";

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

    /// Адреса REST: основной и, если задан, резервный (резерв — только при сбое сети). Адрес — вместе с префиксом
    /// пути API («https://api.gateio.ws/api/v4» — Access URL документации): путь адреса входит в подпись (rest.rs).
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

    /// Символ пары на Gate: «<монета>_<котируемая>», например CAKE_USDT (см. шапку). Ошибка NotConfigured — не задана
    /// venue.quote_asset.
    pub fn symbol(&self, ticker: &str) -> Result<String, GatewayError> {
        Ok(format!("{ticker}_{}", self.text("venue.quote_asset")?))
    }

    /// Десятичное из настроек площадки. Ошибка NotConfigured — не задано.
    pub fn dec(&self, name: &str) -> Result<rust_decimal::Decimal, GatewayError> {
        self.settings
            .decimal(name, self.at())
            .map_err(|e| self.tag(e))
    }

    /// Проставляет в ошибке площадку Gate.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }
}

impl Ctx {
    /// Решение 178: HTTP-клиент REST площадки — с исходящего адреса venue.local_ip, если он задан
    /// (gateway/http_bind.rs); иначе общий. Ошибки: NotConfigured — не IP-адрес; Internal — клиент не собран.
    pub fn client(&self) -> Result<reqwest::Client, crate::gateway::errors::GatewayError> {
        crate::gateway::http_bind::client(&self.http, &self.settings, ID).map_err(|e| self.tag(e))
    }
}
