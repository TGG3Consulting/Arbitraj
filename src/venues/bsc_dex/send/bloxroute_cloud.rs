//! Файл: venues/bsc_dex/send/bloxroute_cloud.rs
//! Что делает: канал bloXroute Cloud API (https://api.blxrbdn.com, заголовок Authorization) — одиночная bsc_private_tx
//! {transaction: raw без 0x}; бандл blxr_submit_bundle {txs без 0x, blockchain_network: BSC-Mainnet, max_block_number};
//! reverting_tx_hashes не передаётся — бандл не включается при откате. Чаевые — BNB на контракт (channel.tip_address).
//! Кто вызывает: send/channels.rs.
//! От чего зависит: send/channel.rs.
//! Настройки: channel.* [bloxroute_cloud]; секрет channel.bloxroute_cloud.key.
//!
//! Источники: https://docs.bloxroute.com/bsc/submit-transactions/bsc-private-transactions.md ,
//! https://docs.bloxroute.com/bsc/submit-bundles/bsc-bundle-submission.md ,
//! https://docs.bloxroute.com/bsc/submit-bundles/bundle-mechanics-and-fees.md (tip 0x6374Ca2da5646C73Eb444aB99780495d61035f9b),
//! https://docs.bloxroute.com/getting-started/authorization-headers.md — research/flow/bsc_builders.md, раздел 2.

use super::channel::{Auth, Bundle, Channel, Single};

/// Значение blockchain_network для BSC из документации bloXroute (bsc-bundle-submission.md).
pub const NETWORK: &str = "BSC-Mainnet";

/// Канал bloXroute Cloud API.
pub const CHANNEL: Channel = Channel {
    id: "bloxroute_cloud",
    title: "bloXroute Cloud API (ключ)",
    private: true,
    single: Some(Single::Bloxroute),
    bundle: Some(Bundle::Bloxroute),
    tips: true,
    auth: Auth::Header,
    docs: &["https://docs.bloxroute.com/bsc/submit-bundles/bsc-bundle-submission.md"],
};
