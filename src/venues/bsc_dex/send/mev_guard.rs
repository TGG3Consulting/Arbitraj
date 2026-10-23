//! Файл: venues/bsc_dex/send/mev_guard.rs
//! Что делает: канал PancakeSwap MEV Guard (Private RPC с bloXroute) — только eth_sendRawTransaction; статус —
//! своим узлом (решение 33). Адрес в документации: https://bscrpc.pancakeswap.finance, Chain ID 56; «completely free».
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [pancake_mev_guard].
//!
//! Источники: https://github.com/pancakeswap/pancake-document/blob/a0a759b15c4a8d374f734f354e5fc72b05ad2200/trading-tools/pancakeswap-mev-guard.md#L1-L27 ,
//! FAQ — …/trading-tools/trading-tools-faq/mev-guard-faq.md#L13-L15. Методы сверх отправки, лимиты, бандлы,
//! защита от отката — в документации не найдены (research/flow/bsc_send.md, раздел 2).

use super::channel::{Auth, Channel, Single};

/// Канал PancakeSwap MEV Guard.
pub const CHANNEL: Channel = Channel {
    id: "pancake_mev_guard",
    title: "PancakeSwap MEV Guard",
    private: true,
    single: Some(Single::Raw),
    bundle: None,
    tips: false,
    auth: Auth::None,
    docs: &["https://docs.pancakeswap.finance/trading-tools/pancakeswap-mev-guard"],
};
