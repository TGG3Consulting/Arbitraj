//! Файл: core/presence.rs
//! Что делает: есть ли монета на одной площадке и в какой роли (часть справочника, шаг 1 ТЗ):
//! у биржи — сеть BSC с адресом контракта, ввод и вывод, пришедший стакан; у DEX — допущенный пул с USDT
//! для того же адреса контракта.
//! Кто вызывает: core/registry.rs (build).
//! От чего зависит: core/types, market_state.rs, registry.rs (Presence, Exclusion, Inputs).
//! Настройки: нет.

use alloy_primitives::Address;

use crate::core::types::{CoinNetworkInfo, VenueId};

use super::market_state::MarketState;
use super::registry::{Exclusion, Inputs, Presence};

/// Монета на бирже: сеть BSC (по адресу контракта, который адаптер ставит только для сети BSC),
/// статусы ввода и вывода, стакан. Ошибок нет.
pub fn cex(
    venue: &VenueId,
    ticker: &str,
    nets: &[&CoinNetworkInfo],
    market: &MarketState,
) -> Presence {
    let bsc = nets.iter().find(|i| i.asset.bsc_contract.is_some());
    let mut excluded = vec![];
    match bsc {
        None => excluded.push(Exclusion::NoBscNetwork),
        Some(_) if market.book(venue, ticker).is_none() => excluded.push(Exclusion::NoSpotBook),
        Some(_) => {}
    }
    Presence {
        venue: venue.clone(),
        network: bsc.map(|i| i.network.clone()),
        can_receive: bsc.is_some_and(|i| i.deposit_enabled),
        can_send: bsc.is_some_and(|i| i.withdraw_enabled),
        excluded,
        info: bsc.map(|i| (*i).clone()),
    }
}

/// Монета на DEX: есть ли допущенный пул с USDT для этого адреса контракта. Ошибок нет.
pub fn dex(venue: &VenueId, token: Address, inp: &Inputs) -> Presence {
    let excluded = match inp.usdt {
        None => vec![Exclusion::NoUsdtAddress],
        Some(usdt) => {
            let pools = inp.market.usdt_pools(venue, token, usdt);
            let bad: Vec<String> = pools
                .iter()
                .filter_map(|p| (inp.pool_ok)(p).err())
                .collect();
            if pools.is_empty() {
                vec![Exclusion::NoUsdtPool]
            } else if bad.len() == pools.len() {
                vec![Exclusion::PoolsExcluded(bad)]
            } else {
                vec![]
            }
        }
    };
    Presence {
        venue: venue.clone(),
        network: None,
        can_receive: true,
        can_send: true,
        excluded,
        info: None,
    }
}
