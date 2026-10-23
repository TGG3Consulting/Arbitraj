//! Файл: venues/bsc_dex/send/flashblock.rs
//! Что делает: канал Flashblock — только бандл: POST <адрес>/proxy, заголовок Authorization, eth_sendBundle
//! {txs, maxBlockNumber}; атомарно; цена газа ≥ 0,05 gwei. Чаевые — BNB на адрес из документации (channel.tip_address).
//! Одиночной отправки в документации нет. Адрес: в документации bsc-eu.flashblock.trade, в списке BNB Chain —
//! us.bsc.flashblock.trade (расхождение — проверяется кнопкой «Проверить подключение»).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [flashblock]; секрет channel.flashblock.key.
//!
//! Источники: https://doc.flashblock.trade/en/BSC/ , https://doc.flashblock.trade/en/SendBundle/
//! (tip 0xfb888290676ba64063ae7b165dae41511aab36ed) — research/flow/bsc_builders.md, раздел 10.

use super::channel::{Auth, Bundle, Channel};

/// Канал Flashblock.
pub const CHANNEL: Channel = Channel {
    id: "flashblock",
    title: "Flashblock (ключ)",
    private: true,
    single: None,
    bundle: Some(Bundle::Standard),
    tips: true,
    auth: Auth::Header,
    docs: &["https://doc.flashblock.trade/en/SendBundle/"],
};
