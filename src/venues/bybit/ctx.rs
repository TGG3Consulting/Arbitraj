//! Файл: venues/bybit/ctx.rs
//! Что делает: общее окружение адаптера Bybit — настройки площадки, секреты, HTTP-клиент, журнал, запись сырых
//! сообщений, приёмники окон (решение 125), состояние ордеров из приватного потока — и чтение настроек площадки в одном
//! месте. Символ спот-пары Bybit: «<монета><котируемая>» без разделителя (BTCUSDT).
//! Кто вызывает: все файлы адаптера Bybit; main.rs (Ctx::new).
//! От чего зависит: settings, secrets, storage (journal, raw_log), gateway/http.rs, private_cache.rs.
//! Настройки: venue.* с областью «bybit» (docs/SETTINGS.md); секреты bybit.api_key, bybit.api_secret.
//!
//! Символ пары — официальная документация «Get Instruments Info» https://bybit-exchange.github.io/docs/v5/market/instrument
//! (пример: symbol «BTCUSDT», baseCoin «BTC», quoteCoin «USDT»; «Symbol name, like BTCUSDT, uppercase only»).

use std::time::Duration;

use zeroize::Zeroizing;

use crate::core::types::VenueId;
use crate::gateway::errors::GatewayError;
use crate::secrets::Vault;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::Journal;
use crate::storage::raw_log::RawLog;

use super::private_cache::Private;

/// Идентификатор площадки. Это имя самого адаптера, по нему ищутся его настройки и секреты.
pub const ID: &str = "bybit";
/// Имя секрета API-ключа (тот же список — secrets::KNOWN, для интерфейса).
pub const SECRET_API_KEY: &str = "bybit.api_key";
/// Имя секрета секретного ключа (подпись HMAC SHA256).
pub const SECRET_API_SECRET: &str = "bybit.api_secret";

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
    /// Ордера и исполнения из приватного потока (order.spot, execution.spot) — итог ордера без лишнего опроса.
    pub private: Private,
}

impl Ctx {
    /// Создаёт окружение. Принимает: хранилище настроек, секреты, HTTP-клиент, журнал, сырой журнал. Ошибок нет.
    pub fn new(
        settings: Store,
        vault: Vault,
        http: reqwest::Client,
        journal: Journal,
        raw: RawLog,
    ) -> Self {
        Self {
            settings,
            vault,
            http,
            journal,
            raw,
            taps: Default::default(),
            private: Default::default(),
        }
    }

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

    /// Адреса REST: основной и, если задан, резервный (документация «Integration Guidance»: «both endpoints are
    /// available» — api.bybit.com и api.bytick.com). Резерв — только при сбое сети. Ошибка NotConfigured.
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

    /// Символ пары: «<монета><котируемая>» (BTCUSDT). Ошибка NotConfigured — не задана venue.quote_asset.
    pub fn symbol(&self, ticker: &str) -> Result<String, GatewayError> {
        Ok(format!("{ticker}{}", self.text("venue.quote_asset")?))
    }

    /// Проставляет в ошибке площадку Bybit.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }
}
