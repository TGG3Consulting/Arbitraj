//! Файл: venues/bsc_dex/send/nodereal.rs
//! Что делает: канал NodeReal Builder (https://bsc-mainnet-builder.nodereal.io; в официальном примере — без ключа):
//! одиночная eth_sendRawTransaction (ответ — объект {txHash}, не строка), бандл eth_sendBundle {txs, maxBlockNumber}.
//! Чаевые — BNB на системный адрес (channel.tip_address). Минимальная цена газа в документации «about 0.1 gwei»
//! (решение 44: channel.min_gas_price_gwei).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [nodereal].
//!
//! Источники: https://docs.nodereal.io/docs/nodereal-builder , https://docs.nodereal.io/docs/send-privatetransaction.md ,
//! https://docs.nodereal.io/docs/send-bundle.md (системный адрес 0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE);
//! пример без ключа — https://github.com/node-real/private-tx-sender/blob/14e502d7f1cf12e2cf1b9c79e2c68f199c843c3a/pkg/builder/nodereal.go
//! (research/flow/bsc_builders.md, раздел 4).

use super::channel::{Auth, Bundle, Channel, Single};

/// Канал NodeReal Builder.
pub const CHANNEL: Channel = Channel {
    id: "nodereal",
    title: "NodeReal Builder",
    private: true,
    single: Some(Single::Raw),
    bundle: Some(Bundle::Standard),
    tips: true,
    auth: Auth::None,
    docs: &["https://docs.nodereal.io/docs/send-bundle.md"],
};
