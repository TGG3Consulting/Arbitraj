//! Файл: venues/bsc_dex/send/public.rs
//! Что делает: канал «открытая отправка» — eth_sendRawTransaction в обычный узел/провайдера (публичный пул).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [public]; адрес — channel.url (узел или провайдер; ключ провайдера в адресе — {key},
//! сам ключ — секрет узла bsc.node_http_key, как у bsc.node_http_url).
//!
//! Метод — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/submit.yaml#L32-L71 ;
//! узел BSC принимает только транзакции с защитой EIP-155 — internal/ethapi/api.go#L2315-L2324 (research/flow/bsc_send.md, раздел 1).
//! Бандлов и чаевых нет; не вместе с закрытыми каналами (решение 33: открытая отправка раскрывает сделку).

use super::channel::{Auth, Channel, Single};

/// Канал открытой отправки.
pub const CHANNEL: Channel = Channel {
    id: "public",
    title: "Открытая отправка (свой узел)",
    private: false,
    single: Some(Single::Raw),
    bundle: None,
    tips: false,
    auth: Auth::NodeKey,
    docs: &[
        "https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/submit.yaml",
    ],
};
