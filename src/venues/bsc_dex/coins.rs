//! Файл: venues/bsc_dex/coins.rs
//! Что делает: справочник монет DEX для контракта coin_networks — монеты, у которых найдены пулы с USDT
//! (достоверные, из реестра пулов), в сети BSC. Кошелёк — свой адрес в сети: принять и отправить токен можно всегда
//! (ERC-20 transfer, https://eips.ethereum.org/EIPS/eip-20); налог на перевод проверяет check/* (шаг 1c),
//! стоимость перевода — withdraw_cost (шаг 1d). Комиссии вывода «как у биржи» у кошелька нет.
//! До subscribe_market реестр пуст, и справочник пуст: ядро сначала подписывается, потом берёт справочник —
//! так подтвердил техлид (Issue #7).
//! Кто вызывает: mod.rs (coin_networks).
//! От чего зависит: ctx.rs, pools/registry.rs.
//! Настройки: venue.bsc_network [bsc_dex] — как называть сеть BSC в справочнике.

use std::collections::BTreeMap;

use chrono::Utc;

use crate::core::types::{Asset, CoinNetworkInfo};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::pools::registry::Registry;

/// Монеты с достоверными пулами. Ошибки: NotConfigured (venue.bsc_network).
pub fn networks(ctx: &Ctx, reg: &Registry) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let network = ctx.text("venue.bsc_network")?;
    let mut coins = BTreeMap::new();
    for r in reg.refs() {
        if let Some(e) = reg.get(&r)
            && e.valid
        {
            coins.insert(e.ticker.clone(), e.coin);
        }
    }
    Ok(coins
        .into_iter()
        .map(|(ticker, addr)| CoinNetworkInfo {
            venue: ctx.venue(),
            asset: Asset {
                ticker,
                bsc_contract: Some(addr),
            },
            network: network.clone(),
            deposit_enabled: true,
            withdraw_enabled: true,
            withdraw_fee: None,
            withdraw_min: None,
            withdraw_max: None,
            deposit_min: None,
            deposit_confirmations: None,
            updated_at: Utc::now(),
        })
        .collect())
}
