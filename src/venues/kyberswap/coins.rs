//! Файл: venues/kyberswap/coins.rs
//! Что делает: справочник монет площадки для контракта coin_networks — монеты активной подписки с адресом контракта в
//! сети BSC (кошелёк принимает и отправляет токен всегда, ERC-20 transfer; комиссий вывода «как у биржи» нет).
//! Вселенная монет приходит со стороны биржи (core/subscribe.rs) — адаптер лишь сообщает сеть BSC по ним. До
//! subscribe_market список пуст.
//! Кто вызывает: venue.rs (coin_networks).
//! От чего зависит: ctx.rs, state.rs, core/types.
//! Настройки: venue.bsc_network [kyberswap] — как называть сеть BSC.

use chrono::Utc;

use crate::core::types::CoinNetworkInfo;
use crate::gateway::errors::GatewayResult;

use super::state::Parts;

/// Монеты активной подписки с адресом в BSC (кроме котируемой монеты). Ошибки: NotConfigured (venue.bsc_network).
pub fn networks(p: &Parts) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let ctx = &p.ctx;
    let network = ctx.venue_text("venue.bsc_network")?;
    let quote = ctx.venue_text("venue.quote_asset").unwrap_or_default();
    Ok(p.assets()
        .into_iter()
        .filter(|a| a.bsc_contract.is_some() && a.ticker != quote)
        .map(|asset| CoinNetworkInfo {
            venue: ctx.venue(),
            asset,
            network: network.clone(),
            deposit_enabled: true,
            withdraw_enabled: true,
            withdraw_fee: None,
            withdraw_min: None,
            withdraw_max: None,
            deposit_min: None,
            deposit_confirmations: None,
            withdraw_precision: None,
            updated_at: Utc::now(),
        })
        .collect())
}
