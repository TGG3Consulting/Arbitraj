//! Файл: venues/bsc_dex/send/blink.rs
//! Что делает: три канала Blink (бывший Merkle, решение 41): два публичных адреса из официальных страниц продукта
//! (bsc.merkle.io и bsc.blink.io) и личный адрес с ключом (bsc.blinklabs.xyz/v1/{key}). Одиночная —
//! eth_sendRawTransaction; бандл — eth_sendBundle {txs}: на BSC пересылается только txs, откат запрещён.
//! Чаевых в документации нет.
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [blink_merkle], [blink_public], [blink_private]; секрет channel.blink_private.key.
//!
//! Источники: https://merkle.io/free-bsc-rpc , https://www.merkle.io/bsc-rpc (до 25 RPS бесплатно),
//! https://docs.blinklabs.xyz/blink/bnb-smart-chain-bsc/api-reference.md (методы, «Only txs is forwarded…»,
//! «no transaction can be opted into reverting») — research/flow/bsc_builders.md, раздел 1.

use super::channel::{Auth, Bundle, Channel, Single};

const DOCS: &[&str] = &[
    "https://docs.blinklabs.xyz/blink/bnb-smart-chain-bsc/api-reference.md",
    "https://merkle.io/free-bsc-rpc",
];

/// Blink, публичный адрес со страницы Merkle (bsc.merkle.io).
pub const MERKLE: Channel = Channel {
    id: "blink_merkle",
    title: "Blink (Merkle), публичный bsc.merkle.io",
    private: true,
    single: Some(Single::Raw),
    bundle: Some(Bundle::TxsOnly),
    tips: false,
    auth: Auth::None,
    docs: DOCS,
};

/// Blink, публичный адрес bsc.blink.io.
pub const PUBLIC: Channel = Channel {
    id: "blink_public",
    title: "Blink, публичный bsc.blink.io",
    private: true,
    single: Some(Single::Raw),
    bundle: Some(Bundle::TxsOnly),
    tips: false,
    auth: Auth::None,
    docs: DOCS,
};

/// Blink, личный адрес с ключом в адресе.
pub const PRIVATE: Channel = Channel {
    id: "blink_private",
    title: "Blink, личный адрес (ключ)",
    private: true,
    single: Some(Single::Raw),
    bundle: Some(Bundle::TxsOnly),
    tips: false,
    auth: Auth::UrlKey,
    docs: DOCS,
};
