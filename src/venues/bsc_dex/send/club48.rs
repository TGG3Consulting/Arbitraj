//! Файл: venues/bsc_dex/send/club48.rs
//! Что делает: канал 48 Club — Privacy RPC (https://rpc.48.club, только одиночная) или Puissant Builder
//! (https://puissant-builder.48.club/, одиночная и бандл) — выбирается адресом channel.url.
//! Одиночная: eth_sendRawTransaction; бандл: eth_sendBundle {txs, maxBlockNumber} без revertingTxHashes —
//! своя транзакция не может откатиться в блоке, бандл с откатом не включается (решение 43).
//! Чаевые: прямой перевод BNB на Builder Control EOA (адрес — channel.tip_address из документации).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [club48].
//!
//! Источники: https://docs.48.club/privacy-rpc.md, https://docs.48.club/puissant-builder/send-privatetransaction.md,
//! https://docs.48.club/puissant-builder/send-bundle (поля, maxBlockNumber по умолчанию +100, tip EOA
//! 0x4848489f0b2BEdd788c696e2D79b6b69D7484848), лимит гостя 50 cond txs / 9 с — …/48-soulpoint-benefits.md;
//! «не слать с одного адреса параллельно» — https://docs.48.club/f.a.q..md (research/flow/bsc_send.md, раздел 3).

use super::channel::{Auth, Bundle, Channel, Single};

/// Канал 48 Club.
pub const CHANNEL: Channel = Channel {
    id: "club48",
    title: "48 Club (Privacy RPC / Puissant)",
    private: true,
    single: Some(Single::Raw),
    bundle: Some(Bundle::Standard),
    tips: true,
    auth: Auth::None,
    docs: &[
        "https://docs.48.club/privacy-rpc.md",
        "https://docs.48.club/puissant-builder/send-bundle",
    ],
};
