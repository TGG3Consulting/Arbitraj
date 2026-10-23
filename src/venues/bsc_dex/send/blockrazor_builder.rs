//! Файл: venues/bsc_dex/send/blockrazor_builder.rs
//! Что делает: канал BlockRazor Block Builder (платный) — заголовок Authorization; одиночная —
//! eth_sendPrivateTransaction; бандл — eth_sendBundle {txs, maxBlockNumber}. Чаевые — перевод BNB на
//! BlockRazor Builder EOA (channel.tip_address). Цена газа не ниже 0,05 gwei (channel.min_gas_price_gwei).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [blockrazor_builder]; секрет channel.blockrazor_builder.key.
//!
//! Источники: https://docs.blockrazor.io/transaction-submission/block-builder/endpoint.md ,
//! …/block-builder/send-bundle.md (tip EOA 0xc7d4187cBda07F4b5be04418B9723b5faF65dB5C), …/send-privatetransaction.md,
//! https://docs.blockrazor.io/authentication.md — research/flow/bsc_send.md, раздел 3.4.

use super::channel::{Auth, Bundle, Channel, Single};

/// Канал BlockRazor Builder.
pub const CHANNEL: Channel = Channel {
    id: "blockrazor_builder",
    title: "BlockRazor Builder (ключ)",
    private: true,
    single: Some(Single::Private),
    bundle: Some(Bundle::Standard),
    tips: true,
    auth: Auth::Header,
    docs: &["https://docs.blockrazor.io/transaction-submission/block-builder/send-bundle.md"],
};
