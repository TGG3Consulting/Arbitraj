//! Файл: core/settlement_market.rs.
//! Что делает: связывает физическую сеть, DEX, актив расчёта и рынок исходной биржи.
//! Кто вызывает: подписки, план, денежные шаги и API.
//! От чего зависит: реестр площадок, подтверждённые записи сетей биржи и настройки DEX.
//! Настройки: dex.chain, dex.settlement_contract, venue.quote_asset, сетевые имена площадок.
use super::{
    engine::Core,
    types::{Asset, ChainId, Direction, VenueId},
};
use crate::settings::{evm_profiles, read::At};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Market {
    pub dex: VenueId,
    pub chain: ChainId,
    pub evm_chain_id: u64,
    pub cex_network: String,
    pub cex: VenueId,
    pub asset: Asset,
    pub quote: String,
}

pub fn mixed(core: &Core, d: &Direction) -> bool {
    let s = &core.inner.deps.settings;
    [(&d.buy_on, &d.sell_on), (&d.sell_on, &d.buy_on)]
        .into_iter()
        .any(|(dex, cex)| {
            evm_profiles::uses_settlement_contract(s, &dex.0)
                && matches!((ticker(core,dex),ticker(core,cex)),(Ok(a),Ok(b)) if a!=b)
        })
}

pub fn ticker(core: &Core, v: &VenueId) -> Result<String, String> {
    core.inner
        .deps
        .settings
        .text(
            "venue.quote_asset",
            At {
                venue: Some(&v.0),
                coin: None,
            },
        )
        .map_err(|e| e.message)
}
pub fn configured(core: &Core, dex: &VenueId) -> Result<Option<Asset>, String> {
    let s = &core.inner.deps.settings;
    if !evm_profiles::uses_settlement_contract(s, &dex.0) {
        if ticker(core, dex).is_ok_and(|v| v != evm_profiles::LEDGER_QUOTE) {
            return Err(format!(
                "{}: для расчётной валюты требуется dex.settlement_contract",
                dex.0
            ));
        }
        return Ok(None);
    }
    let chain = super::chain::chain_at(s, dex)?;
    let ticker = ticker(core, dex)?;
    let contract = evm_profiles::settlement_contract(s, &dex.0, chain.as_str(), &ticker)?;
    let Some(contract) = contract else {
        return Ok(None);
    };
    let tag = super::chain::tag_of_dex(core, dex)?;
    Ok(Some(Asset {
        chain: tag,
        ticker,
        contract: Some(contract),
    }))
}
impl Market {
    pub fn new(core: &Core, dex: &VenueId, cex: &VenueId) -> Result<Self, String> {
        if !super::daily_venue::is_dex(core, &dex.0) || super::daily_venue::is_dex(core, &cex.0) {
            return Err("Требуются площадка DEX и исходная биржа".into());
        }
        let chain = super::chain::chain_at(&core.inner.deps.settings, dex)?;
        let at = At {
            venue: Some(&dex.0),
            coin: None,
        };
        let evm_chain_id = core
            .inner
            .deps
            .settings
            .integer("dex.evm_chain_id", at)
            .map_err(|e| e.message)?;
        let cex_network = crate::gateway::own_nets::name_of(
            &core.inner.deps.settings,
            At {
                venue: Some(&cex.0),
                coin: None,
            },
            Some(&chain),
        )
        .map_err(|e| e.message)?;
        let asset = configured(core, dex)?.ok_or("Не задан контракт расчётного токена DEX")?;
        let quote = ticker(core, cex)?;
        if quote == asset.ticker {
            return Err(
                "Расчётные валюты сторон совпадают, отдельная конверсия не требуется".into(),
            );
        }
        Ok(Self {
            dex: dex.clone(),
            chain,
            evm_chain_id,
            cex_network,
            cex: cex.clone(),
            asset,
            quote,
        })
    }
    /// У полного арбитражного плана результат выражен в валюте портфельного учёта.
    /// Котировка самой конверсии может использовать любую настроенную пару.
    pub fn accounting(&self) -> Result<(), String> {
        if self.quote == evm_profiles::LEDGER_QUOTE {
            Ok(())
        } else {
            Err(format!(
                "Для оценки {} в валюте учёта требуется ещё одна исполнимая конверсия",
                self.quote
            ))
        }
    }
    pub fn direction(core: &Core, d: &Direction) -> Result<Self, String> {
        let (dex, cex) = if super::daily_venue::is_dex(core, &d.buy_on.0) {
            (&d.buy_on, &d.sell_on)
        } else {
            (&d.sell_on, &d.buy_on)
        };
        let m = Self::new(core, dex, cex)?;
        if super::chain::of_core(core, &d.asset)? != m.chain {
            return Err("Сеть монеты отличается от сети расчётного токена".into());
        }
        Ok(m)
    }
    pub fn matches(&self, asset: &Asset) -> bool {
        asset == &self.asset && asset.ticker == self.asset.ticker
    }
    pub fn identical(&self, other: &Self) -> bool {
        self == other && self.asset.ticker == other.asset.ticker
    }
    pub fn ensure(&self, core: &Core) -> Result<(), String> {
        if self.identical(&Self::new(core, &self.dex, &self.cex)?) {
            Ok(())
        } else {
            Err("Изменились сеть, площадка или валюты сохранённой операции".into())
        }
    }
}
