//! Файл: core/settlement_pool.rs.
//! Что делает: определяет физический расчётный ERC-20 площадки для поиска пулов и подписок.
//! Кто вызывает: сборка справочника, подписки и пробуждения промежуточных пар.
//! От чего зависит: адрес из настроек, физическая сеть и справочник хотя бы одной включённой биржи.
//! Настройки: dex.settlement_contract, venue.quote_asset, имена сетей бирж.
use super::{
    engine::{Core, rd},
    registry_build::Networks,
    types::{Asset, VenueId},
};
use alloy_primitives::Address;

pub(super) fn pinned(core: &Core, dex: &VenueId, nets: &Networks) -> Result<Option<Asset>, String> {
    let Some(asset) = super::settlement_market::configured(core, dex)? else {
        return Ok(None);
    };
    let known = nets.iter().any(|(venue, rows)| {
        if !super::registry_sync::enabled(core, venue) {
            return false;
        }
        let at = crate::settings::read::At {
            venue: Some(&venue.0),
            coin: None,
        };
        crate::gateway::own_nets::name_of(&core.inner.deps.settings, at, asset.chain.as_ref())
            .is_ok_and(|name| {
                super::settlement_network::erc20(core, rows, venue, &name, &asset).is_ok()
            })
    });
    if !known {
        return Err(format!(
            "{}: биржи не подтвердили dex.settlement_contract в выбранной сети",
            dex.0
        ));
    }
    Ok(Some(asset))
}

/// Только отсутствие настройки разрешает старому пути брать согласованный адрес из справочника.
pub(super) fn address(
    pinned: Result<Option<Asset>, String>,
    legacy: Option<Address>,
) -> Option<Address> {
    match pinned {
        Ok(Some(asset)) => asset.contract,
        Ok(None) => legacy,
        Err(_) => None,
    }
}

pub(super) fn current(core: &Core, dex: &VenueId) -> Option<Address> {
    let pinned = pinned(core, dex, &rd(&core.inner.networks));
    match pinned {
        Ok(None) => {
            let tag = super::chain::tag_of_dex(core, dex).ok()?;
            super::registry_chains::usdt_of(core, tag.as_ref()).ok()
        }
        other => address(other, None),
    }
}

#[cfg(test)]
#[path = "settlement_pool_tests.rs"]
mod tests;
