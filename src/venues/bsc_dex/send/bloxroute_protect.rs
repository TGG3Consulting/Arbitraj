//! Файл: venues/bsc_dex/send/bloxroute_protect.rs
//! Что делает: канал bloXroute Protect RPC (https://bsc.rpc.blxrbdn.com, «Transactions are not visible in the public
//! mempool») — бесплатный адрес (решение 41); в документации дан только адрес, поэтому — стандартная
//! eth_sendRawTransaction, итог — своим узлом.
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [bloxroute_protect].
//!
//! Источник: https://docs.bloxroute.com/core-solutions/rpc-service/protect-rpc.md (research/flow/bsc_builders.md, раздел 2.1).

use super::channel::{Auth, Channel, Single};

/// Канал bloXroute Protect RPC.
pub const CHANNEL: Channel = Channel {
    id: "bloxroute_protect",
    title: "bloXroute Protect RPC",
    private: true,
    single: Some(Single::Raw),
    bundle: None,
    tips: false,
    auth: Auth::None,
    docs: &["https://docs.bloxroute.com/core-solutions/rpc-service/protect-rpc.md"],
};
