//! Файл: venues/bsc_dex/send/channels.rs
//! Что делает: список каналов отправки адаптера (это перечень его модулей — ARCHITECTURE.md, решение 41) и поиск по id.
//! Решение 435 (433-е): канал секвенсора Arbitrum (sequencer.rs) и сеть канала — serves: channel.chain задана — канал
//! только площадки DEX с той же dex.chain; не задана — канал сети по умолчанию (Ctx::default_chain; площадка DEX одна —
//! все каналы без channel.chain, как до решения 435).
//! Кто вызывает: send/*, mod.rs (send_channels).
//! От чего зависит: файлы каналов этой папки, ctx.rs, ctx_place.rs.
//! Настройки: channel.chain [канал], dex.chain [площадка DEX] (только если у канала задана channel.chain).

use crate::settings::scope::Scope;

use super::super::ctx::Ctx;
use super::channel::Channel;
use super::{
    blink, blockrazor, blockrazor_builder, bloxroute_cloud, bloxroute_protect, club48, flashblock,
};
use super::{jetbuilder, mev_guard, nodereal, nodereal_api, public, sequencer};

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
    &sequencer::CHANNEL,
];

/// Канал по id. None — такого канала нет.
pub fn find(id: &str) -> Option<&'static Channel> {
    ALL.iter().copied().find(|c| c.id == id)
}

/// Решение 435 (433-е): канал отправляет транзакции этой площадки DEX (сеть канала — channel.chain; не задана — сеть
/// по умолчанию). Принимает: окружение, канал. Возвращает: да/нет; нет и при незаданной dex.chain площадки (у канала
/// с сетью) или незаданной dex.default_chain при нескольких площадках DEX (у канала без сети). Ошибок нет.
pub fn serves(ctx: &Ctx, ch: &Channel) -> bool {
    match ctx
        .settings
        .text_in("channel.chain", &Scope::Channel(ch.id.to_string()))
    {
        Ok(chain) => ctx
            .text("dex.chain")
            .is_ok_and(|own| own.trim() == chain.trim()),
        Err(_) => ctx.default_chain().unwrap_or(false),
    }
}

/// Каналы этой площадки DEX (serves) по порядку ALL. Принимает: окружение. Ошибок нет.
pub fn of(ctx: &Ctx) -> impl Iterator<Item = &'static Channel> + '_ {
    ALL.iter().copied().filter(move |c| serves(ctx, c))
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
