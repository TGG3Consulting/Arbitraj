//! Класс продукта Bitget на один снимок V2/V3. Пропуск метки сохраняет у известной
//! пары только остаток срока исходного подтверждения. Отдельные часы отвечают за TTL и журнал.

use super::symbols::SymbolRules;
use crate::core::types::Asset;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) struct Stamp {
    pub instant: Instant,
    pub utc: DateTime<Utc>,
}

impl Stamp {
    pub fn now() -> Self {
        Self {
            instant: Instant::now(),
            utc: Utc::now(),
        }
    }
}

#[derive(Clone, Copy)]
struct Previous {
    confirmed: Stamp,
    until: Instant,
}

pub(super) struct Missing {
    pub rules: SymbolRules,
    omitted: bool,
    previous: Option<Previous>,
}

impl Missing {
    pub fn new(rules: SymbolRules, omitted: bool) -> Self {
        Self {
            rules,
            omitted,
            previous: None,
        }
    }

    pub fn reason(&self) -> String {
        if let Some(previous) = self.previous {
            format!(
                "Bitget не подтвердил класс пары {}. Последнее подтверждение Classic: {}",
                self.rules.symbol, previous.confirmed.utc
            )
        } else {
            format!("Bitget не подтвердил класс пары {}", self.rules.symbol)
        }
    }
}

#[derive(Default)]
pub(super) struct Catalog {
    pub classic: Arc<HashMap<String, SymbolRules>>,
    pub reality: HashSet<String>,
    pub missing_class: HashMap<String, Missing>,
}

impl Catalog {
    /// Следующий снимок наследует первоначальное подтверждение только при точном совпадении V2.
    pub fn retain_known(&mut self, old: &Self, old_stamp: Stamp, limit: Option<Duration>) {
        let Some(limit) = limit else { return };
        for (ticker, unknown) in &mut self.missing_class {
            if !unknown.omitted {
                continue;
            }
            let prior = old
                .classic
                .get(ticker)
                .map(|rules| (rules, old_stamp, None))
                .or_else(|| {
                    let previous = old.missing_class.get(ticker)?;
                    let seen = previous.previous?;
                    Some((&previous.rules, seen.confirmed, Some(seen.until)))
                });
            let Some((rules, confirmed, previous_deadline)) = prior else {
                continue;
            };
            if (rules.symbol.as_str(), rules.quote.as_str())
                != (unknown.rules.symbol.as_str(), unknown.rules.quote.as_str())
            {
                continue;
            }
            if let Some(until) = confirmed.instant.checked_add(limit) {
                unknown.previous = Some(Previous {
                    confirmed,
                    until: previous_deadline.map_or(until, |d| d.min(until)),
                });
            }
        }
    }

    pub fn rule(&self, ticker: &str, now: Instant) -> Option<&SymbolRules> {
        self.classic.get(ticker).or_else(|| {
            let unknown = self.missing_class.get(ticker)?;
            (now < unknown.previous?.until).then_some(&unknown.rules)
        })
    }

    pub fn state(
        &self,
        ticker: &str,
        now: Instant,
    ) -> Option<crate::gateway::events::SpotPairState> {
        if let Some(rules) = self.rule(ticker, now) {
            return Some(rules.state(ticker));
        }
        let unknown = self.missing_class.get(ticker)?;
        let mut state = unknown.rules.state(ticker);
        state.order_block = Some(unknown.reason());
        Some(state)
    }

    pub fn has_book(&self, asset: &Asset, symbol: &str, now: Instant) -> bool {
        self.rule(&asset.ticker, now)
            .is_some_and(|r| r.tradable() && r.symbol == symbol)
    }

    pub fn expiry(&self, now: Instant) -> Option<Instant> {
        self.missing_class
            .values()
            .filter_map(|u| u.previous.map(|p| p.until))
            .filter(|d| *d > now)
            .min()
    }
}
