//! Файл: core/presence.rs
//! Что делает: есть ли монета на одной площадке и в какой роли (часть справочника, шаг 1 ТЗ):
//! у биржи — сеть BSC с адресом контракта, ввод и вывод, пара торгуется (статус, покупка и продажа через API),
//! пришедший стакан; у DEX — допущенный пул с USDT
//! для того же адреса контракта или — если включено (решение 52) — пул с промежуточной монетой из списка.
//! Кто вызывает: core/registry.rs (build).
//! От чего зависит: core/types, market_state.rs, registry.rs (Presence, Exclusion, Inputs).
//! Настройки: нет своих (название сети BSC — venue.bsc_network — читает rebuild.rs).

use alloy_primitives::Address;

use crate::core::types::{CoinNetworkInfo, VenueId};

use super::intermediate::Intermediate;
use super::market_state::MarketState;
use super::registry::{Exclusion, Inputs, Presence};

/// Монета на бирже: сеть BSC и адрес контракта (адаптер ставит адрес только для сети BSC; сеть узнаётся по
/// названию bsc — venue.bsc_network), статусы ввода и вывода, состояние спот-пары (MarketEvent::SpotPairs) и стакан.
/// Причины по порядку: нет сети BSC → сеть есть, адреса нет → пары нет (или справочник пар ещё не пришёл) →
/// справочнику пар верить нельзя (fresh: устарел, порог не задан — pairs_fresh.rs) → пара не торгуется по статусу →
/// через API нельзя ни купить, ни продать → стакан не пришёл.
/// Покупка и продажа через API по отдельности закрывают свою сторону (can_buy / can_sell), а не всю монету.
/// Принимает: биржу, тикер, сети монеты, название сети BSC (None — не задано), свежесть справочника пар, рынок.
/// Ошибок нет.
pub fn cex(
    venue: &VenueId,
    ticker: &str,
    nets: &[&CoinNetworkInfo],
    bsc: Option<&str>,
    fresh: &Result<(), String>,
    market: &MarketState,
) -> Presence {
    let with_contract = nets.iter().find(|i| i.asset.bsc_contract.is_some());
    let net = with_contract.or_else(|| nets.iter().find(|i| Some(i.network.as_str()) == bsc));
    let pair = market.pair(venue, ticker);
    let excluded = match (net, with_contract, pair) {
        (None, _, _) => vec![Exclusion::NoBscNetwork],
        (Some(_), None, _) => vec![Exclusion::NoContract],
        (_, _, None) => vec![Exclusion::NoSpotBook],
        _ if fresh.is_err() => vec![Exclusion::PairsStale(
            fresh.clone().err().unwrap_or_default(),
        )],
        (_, _, Some(p)) if !p.online => vec![Exclusion::PairNotTrading(p.status)],
        (_, _, Some(p)) if !p.api_buy && !p.api_sell => vec![Exclusion::NoApiTrading],
        _ if market.book(venue, ticker).is_none() => vec![Exclusion::NoSpotBook],
        _ => vec![],
    };
    Presence {
        venue: venue.clone(),
        network: net.map(|i| i.network.clone()),
        can_receive: net.is_some_and(|i| i.deposit_enabled),
        can_send: net.is_some_and(|i| i.withdraw_enabled),
        can_buy: pair.is_some_and(|p| p.api_buy),
        can_sell: pair.is_some_and(|p| p.api_sell),
        excluded,
        info: with_contract.map(|i| (*i).clone()),
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
        can_buy: true,
        can_sell: true,
        excluded,
        info: None,
        via,
    }
}

/// Есть ли допущенный пул пары a–b. Ошибка — причина: пулов нет (NoUsdtPool) или все исключены.
fn admitted(venue: &VenueId, a: Address, b: Address, inp: &Inputs) -> Result<(), Exclusion> {
    let pools = inp.market.usdt_pools(venue, a, b);
    let bad: Vec<String> = pools
        .iter()
        .filter_map(|p| (inp.pool_ok)(p).err())
        .collect();
    if pools.is_empty() {
        Err(Exclusion::NoUsdtPool)
    } else if bad.len() == pools.len() {
        Err(Exclusion::PoolsExcluded(bad))
    } else {
        Ok(())
    }
}
