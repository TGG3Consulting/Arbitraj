//! Файл: venues/bsc_dex/ctx_url.rs
//! Что делает: адрес узла BSC из настройки с подстановкой ключа провайдера вместо {key} (ключ — секрет, в журнал адрес
//! не пишется) — часть окружения адаптера (ctx.rs).
//! Кто вызывает: node/rpc.rs, node/ws.rs, node/failover.rs, connection.rs, send/*.
//! От чего зависит: ctx.rs (Ctx, KEY_MARK), secrets (Vault), settings.
//! Настройки: адреса узла (bsc.node_http_url, bsc.node_ws_url и резервные) — имя передаёт вызывающий.

use zeroize::Zeroizing;

use crate::gateway::errors::GatewayError;
use crate::settings::read::At;

use super::ctx::{Ctx, KEY_MARK};

impl Ctx {
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
}
