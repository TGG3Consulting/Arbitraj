//! Файл: core/presence.rs
//! Что делает: есть ли монета на одной площадке и в какой роли (часть справочника, шаг 1 ТЗ):
//! у биржи — сеть BSC с адресом контракта, ввод и вывод, пришедший стакан; у DEX — допущенный пул с USDT
//! для того же адреса контракта или — если включено (решение 52) — пул с промежуточной монетой из списка.
//! Кто вызывает: core/registry.rs (build).
//! От чего зависит: core/types, market_state.rs, registry.rs (Presence, Exclusion, Inputs).
//! Настройки: нет.

use alloy_primitives::Address;

use crate::core::types::{CoinNetworkInfo, VenueId};

use super::intermediate::Intermediate;
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
        via: None,
    }
}

/// Монета на DEX: допущенный пул с USDT для этого адреса контракта; если его нет и промежуточные монеты
/// включены (решение 52) — допущенный пул с промежуточной монетой, у которой самой есть допущенный пул
/// с USDT (иначе маршрут «USDT → промежуточная → монета» не собрать). Ошибок нет.
pub fn dex(venue: &VenueId, token: Address, inp: &Inputs) -> Presence {
    let mut via = None;
    let excluded = match inp.usdt {
        None => vec![Exclusion::NoUsdtAddress],
        Some(usdt) => match admitted(venue, token, usdt, inp) {
            Ok(()) => vec![],
            Err(direct) => match inp.intermediate.get(venue) {
                Some(Intermediate::On(list)) => {
                    let found = list.iter().find(|m| {
                        admitted(venue, token, **m, inp).is_ok()
                            && admitted(venue, **m, usdt, inp).is_ok()
                    });
                    match found {
                        Some(m) => {
                            via = Some(*m);
                            vec![]
                        }
                        None => vec![direct],
                    }
                }
                Some(Intermediate::NotConfigured(why)) => {
                    vec![direct, Exclusion::IntermediateNotConfigured(why.clone())]
                }
                _ => vec![direct],
            },
        },
    };
    Presence {
        venue: venue.clone(),
        network: None,
        can_receive: true,
        can_send: true,
        excluded,
        info: None,
        via,
    }
}

/// Есть ли допущенный пул пары a–b. Ошибка — причина: пулы найдены, но не прочитаны (PoolsLoading), пулов нет
/// (NoUsdtPool) или все исключены.
fn admitted(venue: &VenueId, a: Address, b: Address, inp: &Inputs) -> Result<(), Exclusion> {
    let pools = inp.market.usdt_pools(venue, a, b);
    let bad: Vec<String> = pools
        .iter()
        .filter_map(|p| (inp.pool_ok)(p).err())
        .collect();
    if pools.is_empty() && inp.market.pools_loading(venue, a) {
        Err(Exclusion::PoolsLoading)
    } else if pools.is_empty() {
        Err(Exclusion::NoUsdtPool)
    } else if bad.len() == pools.len() {
        Err(Exclusion::PoolsExcluded(bad))
    } else {
        Ok(())
    }
}
