//! Файл: venues/bsc_dex/send/channels.rs
//! Что делает: список каналов отправки адаптера (это перечень его модулей — ARCHITECTURE.md, решение 41) и поиск по id.
//! Кто вызывает: send/*, mod.rs (send_channels).
//! От чего зависит: файлы каналов этой папки.
//! Настройки: нет.

use super::channel::Channel;
use super::{
    blink, blockrazor, blockrazor_builder, bloxroute_cloud, bloxroute_protect, club48, flashblock,
};
use super::{jetbuilder, mev_guard, nodereal, nodereal_api, public};

/// Все каналы — порядок как в ARCHITECTURE.md.
pub static ALL: &[&Channel] = &[
    &public::CHANNEL,
    &mev_guard::CHANNEL,
    &club48::CHANNEL,
    &blockrazor::CHANNEL,
    &blockrazor_builder::CHANNEL,
    &blink::MERKLE,
    &blink::PUBLIC,
    &blink::PRIVATE,
    &jetbuilder::CHANNEL,
    &nodereal::CHANNEL,
    &nodereal_api::CHANNEL,
    &bloxroute_protect::CHANNEL,
    &bloxroute_cloud::CHANNEL,
    &flashblock::CHANNEL,
];

/// Канал по id. None — такого канала нет.
pub fn find(id: &str) -> Option<&'static Channel> {
    ALL.iter().copied().find(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// У каждого канала с ключом есть секрет из secrets::KNOWN (стык).
    #[test]
    fn secrets_are_known() {
        for c in ALL {
            if let Some(s) = c.secret() {
                assert!(crate::secrets::KNOWN.iter().any(|(n, _, _)| *n == s), "{s}");
            }
        }
    }
}
