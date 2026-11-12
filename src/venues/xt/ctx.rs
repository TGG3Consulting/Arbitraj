//! Файл: venues/xt/ctx.rs
//! Что делает: общее окружение адаптера XT.com — настройки площадки, секреты, HTTP-клиент, журнал, запись сырых
//! сообщений — и чтение настроек площадки в одном месте.
//! Кто вызывает: все файлы адаптера XT, main.rs (создаёт).
//! От чего зависит: settings, secrets, storage (journal, raw_log), core/types.
//! Настройки: venue.* с областью «xt» (docs/SETTINGS.md, раздел «Площадки — XT»); секреты xt.api_key, xt.api_secret.
//!
//! Официально (https://doc.xt.com/docs/spot/Access%20Description/SignatureInstructions, исходник github.com/xt-com/xt4-api
//! `_documentation/signStatement.md`): ключ — заголовок validate-appkey, подпись — validate-signature (HMAC SHA256
//! секретом ключа, hex). Passphrase у XT нет. Имена монет у XT строчные («usdt», «btc» — `_balance/balancesGet.md`,
//! «Currency name must be all lowercase» — `_transfer/transferPost.md`); ядру отдаётся тикер заглавными (CAKE).

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
pub const ID: &str = "xt";
/// Имя секрета API-ключа (тот же список — secrets::KNOWN, для интерфейса).
pub const SECRET_API_KEY: &str = "xt.api_key";
/// Имя секрета секретного ключа (подпись HMAC SHA256).
pub const SECRET_API_SECRET: &str = "xt.api_secret";

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

    /// Адрес REST спота (venue.rest_url, официально https://sapi.xt.com) без «/» в конце. Ошибка NotConfigured.
    pub fn rest_url(&self) -> Result<String, GatewayError> {
        Ok(self
            .text("venue.rest_url")?
            .trim_end_matches('/')
            .to_string())
    }

    /// API-ключ и секрет. Ошибки: SecretsLocked, NotConfigured (секрет не введён).
    pub fn keys(&self) -> Result<(Zeroizing<String>, Zeroizing<String>), GatewayError> {
        let key = self.vault.get(SECRET_API_KEY).map_err(|e| self.tag(e))?;
        let secret = self.vault.get(SECRET_API_SECRET).map_err(|e| self.tag(e))?;
        Ok((key, secret))
    }

    /// Проставляет в ошибке площадку XT. Ошибок нет.
    pub fn tag(&self, mut e: GatewayError) -> GatewayError {
        e.venue = Some(self.venue());
        e
    }

    /// Запись в журнал от имени XT. Принимает: уровень, код записи, текст. Ошибок нет (сбой журнала не мешает).
    pub async fn log(&self, level: Level, code: &str, msg: String) {
        let _ = self.journal.write(level, code, Some(ID), msg, None).await;
    }
}

/// Тикер ядра по имени монеты XT: заглавными («cake» → «CAKE»). Ошибок нет.
pub fn ticker(xt_currency: &str) -> String {
    xt_currency.trim().to_uppercase()
}

/// Имя монеты XT по тикеру ядра: строчными (документация: «Currency name must be all lowercase»). Ошибок нет.
pub fn currency(ticker: &str) -> String {
    ticker.trim().to_lowercase()
}

impl Ctx {
    /// Решение 178: HTTP-клиент REST площадки — с исходящего адреса venue.local_ip, если он задан
    /// (gateway/http_bind.rs); иначе общий. Ошибки: NotConfigured — не IP-адрес; Internal — клиент не собран.
    pub fn client(&self) -> Result<reqwest::Client, crate::gateway::errors::GatewayError> {
        crate::gateway::http_bind::client(&self.http, &self.settings, ID).map_err(|e| self.tag(e))
    }
}
