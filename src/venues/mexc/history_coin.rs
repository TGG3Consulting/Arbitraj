//! Имена монет из истории MEXC подтверждает справочник монеты и конкретной сети.
//! Ключ включает сеть: суффиксы имени не являются контрактом API.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use crate::core::types::{CostKind, Transfer, TransferStatus};

#[derive(PartialEq)]
struct Identity {
    ticker: String,
    code: String,
    contract: Option<String>,
}

#[derive(Default)]
pub(super) struct HistoryCoins {
    networks: HashMap<String, HashMap<String, Option<Arc<Identity>>>>,
    aliases: HashSet<String>,
}

impl HistoryCoins {
    /// Разрешается только единственное соответствие, включая совпадение alias с именем другой монеты.
    pub(super) fn insert(&mut self, ticker: &str, code: &str, display: &str, row: &Value) {
        let id = Arc::new(Identity {
            ticker: ticker.to_owned(),
            code: code.to_owned(),
            contract: row["contract"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        });
        let parent_matches = row["coin"].as_str().is_none_or(|coin| coin == ticker);
        if let Some(alias) = row["name"]
            .as_str()
            .filter(|s| !s.is_empty() && *s != ticker)
        {
            self.aliases.insert(alias.to_owned());
        }
        for network in [code, display].into_iter().filter(|s| !s.is_empty()) {
            let by_coin = self.networks.entry(network.to_owned()).or_default();
            for name in [Some(ticker), row["name"].as_str()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
            {
                let slot = by_coin
                    .entry(name.to_owned())
                    .or_insert_with(|| Some(id.clone()));
                if !parent_matches || slot.as_deref() != Some(&id) {
                    *slot = None;
                }
            }
        }
    }

    fn identity(&self, network: &str, raw: &str) -> Option<&Identity> {
        self.networks.get(network)?.get(raw)?.as_deref()
    }

    /// Неизвестное имя без свидетельств сетевого псевдонима оценивается прежним путём по сырому coin.
    /// Для известных псевдонимов нужна их точная сеть. Конфликт строк блокирует даже совпадающий тикер.
    pub(super) fn hold_ticker<'a>(&'a self, network: &str, raw: &'a str) -> Option<&'a str> {
        match self.networks.get(network).and_then(|rows| rows.get(raw)) {
            Some(Some(id)) => Some(&id.ticker),
            Some(None) => None,
            None if self.aliases.contains(raw) => None,
            None => Some(raw),
        }
    }

    #[cfg(test)]
    fn ticker(&self, network: &str, raw: &str) -> Option<&str> {
        Some(self.identity(network, raw)?.ticker.as_str())
    }

    /// Сырое имя монеты в ответе остаётся на Transfer. Меняем только доказанную валюту комиссии.
    pub(super) fn withdrawal_fee(&self, created: &Transfer, sent: &mut Transfer) {
        let Some(id) = self.identity(&sent.network, &sent.asset.ticker) else {
            return;
        };
        let Some(fee) = &mut sent.fee else { return };
        if !created.venue_ref.is_empty()
            && created.venue_ref == sent.venue_ref
            && created.venue == sent.venue
            && sent.status == TransferStatus::Sent
            && sent.tx_hash.as_deref().is_some_and(|h| !h.is_empty())
            && created.asset.ticker == id.ticker
            && created.network == id.code
            && self.identity(&created.network, &created.asset.ticker) == Some(id)
            && created
                .asset
                .contract
                .is_none_or(|a| id.contract.as_deref().and_then(|s| s.parse().ok()) == Some(a))
            && fee.kind == CostKind::WithdrawFee
            && fee.asset == sent.asset.ticker
            && fee.paid_in.is_none()
            && !fee.in_price
        {
            fee.asset.clone_from(&created.asset.ticker);
        }
    }
}

#[cfg(test)]
#[path = "history_coin_tests.rs"]
mod tests;
