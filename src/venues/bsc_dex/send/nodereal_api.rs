//! Файл: venues/bsc_dex/send/nodereal_api.rs
//! Что делает: канал NodeReal Bundle Service API (ключ в адресе bsc-mainnet.nodereal.io/{key}, тариф Growth+) —
//! только бандл eth_sendBundle {txs, maxBlockNumber} (ответ — массив). «If a single transaction within a bundle fails,
//! the entire bundle is guaranteed not to be packaged». Чаевые — как у NodeReal Builder (channel.tip_address).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [nodereal_api]; секрет channel.nodereal_api.key.
//!
//! Источник: https://docs.nodereal.io/reference/bsc-bundle-service-api (research/flow/bsc_builders.md, раздел 4.2).

use super::channel::{Auth, Bundle, Channel};

/// Канал NodeReal Bundle Service API.
pub const CHANNEL: Channel = Channel {
    id: "nodereal_api",
    title: "NodeReal Bundle Service API (ключ)",
    private: true,
    single: None,
    bundle: Some(Bundle::Standard),
    tips: true,
    auth: Auth::UrlKey,
    docs: &["https://docs.nodereal.io/reference/bsc-bundle-service-api"],
};
