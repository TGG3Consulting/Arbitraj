//! Файл: venues/bsc_dex/send/blockrazor.rs
//! Что делает: канал BlockRazor RPC (бесплатный, без ключа) — eth_sendRawTransaction; режим (default / fullprivacy /
//! maxbackrun) — путь в channel.url. Бандл через RPC не подключён: документация противоречит сама себе (в заголовке
//! eth_sendBundle, в примере eth_sendMevBundle; поле txs/Txs) — вопрос Тиграну в Issue #7.
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [blockrazor].
//!
//! Источники: https://docs.blockrazor.io/transaction-submission/rpc/bsc/bsc-rpc-endpoint.md (адреса и режимы),
//! …/rpc/bsc/integration.md («free to all users. No auth needed»), …/rpc/bsc/eth_sendrawtransaction.md,
//! …/rpc/bsc/eth_sendbundle.md (противоречие) — research/flow/bsc_send.md, раздел 3.3.

use super::channel::{Auth, Channel, Single};

/// Канал BlockRazor RPC.
pub const CHANNEL: Channel = Channel {
    id: "blockrazor",
    title: "BlockRazor RPC",
    private: true,
    single: Some(Single::Raw),
    bundle: None,
    tips: false,
    auth: Auth::None,
    docs: &["https://docs.blockrazor.io/transaction-submission/rpc/bsc/bsc-rpc-endpoint.md"],
};
