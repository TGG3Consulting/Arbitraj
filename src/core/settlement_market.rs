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
    /// Отсутствие означает совпавшие символы. Старые ключи с такими символами сохраняют исходный JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cex_asset_symbol: Option<String>,
    pub quote: String,
}

/// Денежный круг конверсии у направления: у стороны с dex.settlement_contract расчётная валюта отличается от валюты
/// биржи-напарника. Равные имена USDT — см. cash_same.rs. Принимает: ядро, направление. Ошибок нет.
pub fn mixed(core: &Core, d: &Direction) -> bool {
    let s = &core.inner.deps.settings;
    [(&d.buy_on, &d.sell_on), (&d.sell_on, &d.buy_on)]
        .into_iter()
        .any(|(dex, cex)| {
            if !evm_profiles::uses_settlement_contract(s, &dex.0) {
                return false;
            }
            let (Ok(a), Ok(b)) = (ticker(core, dex), ticker(core, cex)) else {
                return false;
            };
            if a != b || b != evm_profiles::LEDGER_QUOTE {
                return true;
            }
            // Равные имена USDT (найдено арбитром 28.11.2026, cash_same.rs): адрес USDT сети подтверждён — прямой
            // расчёт. Не подтверждён — денежный круг только если биржа зовёт закреплённый ERC-20 иначе (Market::new
            // проходит); иначе прямой путь, и сигнал со входом отвечают явной причиной (cash_same::check), а не вечным
            // «Расчётные валюты сторон совпадают».
            if super::cash_same::confirmed(core, dex).is_ok() {
                return false;
            }
            Market::new(core, dex, cex).is_ok()
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
        let asset = configured(core, dex)?.ok_or("Не задан контракт расчётного токена DEX")?;
        let cex_network = crate::gateway::own_nets::name_of(
            &core.inner.deps.settings,
            At {
                venue: Some(&cex.0),
                coin: None,
            },
            asset.chain.as_ref(),
        )
        .map_err(|e| e.message)?;
        let rows = super::engine::rd(&core.inner.networks);
        let row = super::settlement_network::erc20(
            core,
            rows.get(cex)
                .map(|rows| rows.as_slice())
                .unwrap_or_default(),
            cex,
            &cex_network,
            &asset,
        )?;
        let cex_asset_symbol = cash_alias(&asset, &row.asset);
        drop(rows);
        let quote = ticker(core, cex)?;
        if quote == cex_asset_symbol.as_deref().unwrap_or(&asset.ticker) {
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
            cex_asset_symbol,
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
    pub(super) fn balance_key(&self, core: &Core, asset: &Asset, venue: &VenueId) -> String {
        if venue == &self.dex && super::native::is_native(core, asset) {
            asset.ticker.clone()
        } else {
            super::venue_ticker::balance_key(core, asset, venue)
        }
    }
    pub(super) fn cash_on(&self, venue: &VenueId) -> Asset {
        let mut on = self.asset.clone();
        if venue == &self.cex {
            on.ticker = self.cex_symbol().to_string();
        }
        on
    }
    pub fn cex_symbol(&self) -> &str {
        self.cex_asset_symbol
            .as_deref()
            .unwrap_or(&self.asset.ticker)
    }
    pub(super) fn book_key(&self) -> (VenueId, String) {
        (self.cex.clone(), self.cex_symbol().to_string())
    }
    /// Биржа подтверждает символ расчётной монеты своим справочником даже если самой монеты нет среди целей.
    pub(super) fn asset_on(&self, core: &Core, token: &Asset, venue: &VenueId) -> Asset {
        if token == &self.asset {
            self.cash_on(venue)
        } else {
            super::venue_ticker::on_venue(core, token, venue)
        }
    }
    pub fn matches(&self, asset: &Asset) -> bool {
        asset == &self.asset && asset.ticker == self.cex_symbol()
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

fn cash_alias(dex: &Asset, cex: &Asset) -> Option<String> {
    (cex.ticker != dex.ticker).then(|| cex.ticker.clone())
}
#[cfg(test)]
#[path = "settlement_alias_tests.rs"]
mod tests;
