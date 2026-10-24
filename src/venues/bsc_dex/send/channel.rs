//! Файл: venues/bsc_dex/send/channel.rs
//! Что делает: общий вид канала отправки (решения 33, 41): что канал умеет по своей официальной документации —
//! одиночная отправка (каким методом), бандл (в каком формате), закрытый ли канал, есть ли чаевые, нужен ли ключ
//! и куда он ставится (в адрес или в заголовок Authorization). Сами каналы — по файлу на канал в этой папке.
//! Кто вызывает: send/*.
//! От чего зависит: core/types (SendChannelInfo, SendMethod).
//! Настройки: нет (настройки канала — channel.* с областью «канал»).

use crate::core::types::{SendChannelInfo, SendMethod};

/// Одиночная отправка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Single {
    /// eth_sendRawTransaction(raw) — ответ: хеш строкой (или объект у NodeReal builder).
    Raw,
    /// eth_sendPrivateTransaction(raw).
    Private,
    /// bloXroute Cloud API: bsc_private_tx {transaction: raw без 0x}.
    Bloxroute,
}

/// Бандл. Только с пределом блока (maxBlockNumber): бандл без предела не истекает сам (см. send/blink.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bundle {
    /// eth_sendBundle {txs, maxBlockNumber} (48 Club, BlockRazor Builder, JetBuilder, NodeReal, Flashblock).
    Standard,
    /// bloXroute Cloud API: blxr_submit_bundle {txs без 0x, blockchain_network, max_block_number}.
    Bloxroute,
}

/// Куда ставится ключ канала.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// Ключ не нужен.
    None,
    /// Ключ в адресе вместо {key} (channel.url).
    UrlKey,
    /// Заголовок Authorization: <ключ>.
    Header,
    /// Канал идёт через узел BSC (открытая отправка): {key} в channel.url — ключ провайдера узла, секрет
    /// bsc.node_http_key (как в bsc.node_http_url); метки нет — адрес как есть. Своего ключа у канала нет.
    NodeKey,
}

/// Описание канала по его официальной документации.
#[derive(Debug)]
pub struct Channel {
    pub id: &'static str,
    pub title: &'static str,
    /// Закрытый канал (не публичный пул).
    pub private: bool,
    pub single: Option<Single>,
    pub bundle: Option<Bundle>,
    /// Есть ли у канала чаевые билдеру по документации.
    pub tips: bool,
    pub auth: Auth,
    pub docs: &'static [&'static str],
}

impl Channel {
    /// Имя секрета ключа канала (secrets::KNOWN: channel.<id>.key); None — ключ не нужен.
    pub fn secret(&self) -> Option<String> {
        matches!(self.auth, Auth::UrlKey | Auth::Header).then(|| format!("channel.{}.key", self.id))
    }

    /// Описание для ядра и интерфейса. Ошибок нет.
    pub fn info(&self) -> SendChannelInfo {
        let mut methods = Vec::new();
        if self.single.is_some() {
            methods.push(SendMethod::Single);
        }
        if self.bundle.is_some() {
            methods.push(SendMethod::Bundle);
        }
        SendChannelInfo {
            id: self.id.to_string(),
            title: self.title.to_string(),
            methods,
            private: self.private,
            tips: self.tips,
            secret: self.secret(),
            docs: self.docs.iter().map(|s| s.to_string()).collect(),
        }
    }
}
