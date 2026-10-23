//! Файл: venues/bsc_dex/send/jetbuilder.rs
//! Что делает: канал JetBuilder (https://rpc.bsc.jetbldr.xyz, ключ не упоминается) — одиночная eth_sendRawTransaction
//! («never broadcast to the public mempool»), бандл eth_sendBundle {txs, maxBlockNumber}. Чаевые — прямой перевод
//! BNB на кошелёк билдера (channel.tip_address). Других методов у адреса нет (eth_chainId недоступен).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [jetbuilder].
//!
//! Источники: https://jetbldr.xyz/api_bsc/ (адрес, «method … does not exist», tip 0xe89c42BC188c993273Ab34231e12ae60c73042E4),
//! http://jetbldr.xyz/api_bsc/eth_sendPrivateRawTransaction/ , https://jetbldr.xyz/api_bsc/eth_sendbundle —
//! research/flow/bsc_builders.md, раздел 3.

use super::channel::{Auth, Bundle, Channel, Single};

/// Канал JetBuilder.
pub const CHANNEL: Channel = Channel {
    id: "jetbuilder",
    title: "JetBuilder",
    private: true,
    single: Some(Single::Raw),
    bundle: Some(Bundle::Standard),
    tips: true,
    auth: Auth::None,
    docs: &["https://jetbldr.xyz/api_bsc/"],
};
