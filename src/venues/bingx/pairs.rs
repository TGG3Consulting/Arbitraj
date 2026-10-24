//! Файл: venues/bingx/pairs.rs
//! Что делает: снимок пар монет подписки по справочнику пар (symbols.rs): на какие стаканы подписываться,
//! каких пар нет, какие монеты торгуются (для подписки DEX в ядре) и состояние каждой пары (статус, покупка и
//! продажа через API) — событие ядру MarketEvent::SpotPairs.
//! Кто вызывает: market.rs (при подписке), pairs_watch.rs (при обновлении справочника пар).
//! От чего зависит: symbols.rs, gateway/events, core/types.
//! Настройки: нет.
//!
//! Официальная документация: «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols —
//! status (торгуется только 1), apiStateBuy, apiStateSell.

use crate::core::types::{Asset, VenueId};
use crate::gateway::events::{MarketEvent, SpotPairState};

use super::symbols::Symbols;

/// Снимок пар монет подписки.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Стаканы для подписки: монета и символ BingX — пары со статусом «торгуется» (status 1). Стакан нужен
    /// и паре, закрытой для API: его цена — живая цена монеты (газ, промежуточные монеты, core/convert.rs).
    pub books: Vec<(Asset, String)>,
    /// Монеты без пары со статусом 1 (для журнала).
    pub missing: Vec<String>,
    /// Монеты, которыми можно торговать через API хотя бы в одну сторону (status 1 и apiStateBuy или apiStateSell).
    pub tickers: Vec<String>,
    /// Состояние пары каждой монеты, у которой пара есть в справочнике.
    pub pairs: Vec<SpotPairState>,
}

impl Snapshot {
    /// Строит снимок по справочнику пар. Принимает: справочник, монеты подписки. Ошибок нет: монета без пары
    /// попадает в missing и в pairs не входит.
    pub fn build(symbols: &Symbols, assets: &[Asset]) -> Self {
        let mut s = Snapshot {
            books: vec![],
            missing: vec![],
            tickers: vec![],
            pairs: vec![],
        };
        for a in assets {
            let Some(p) = symbols.state(&a.ticker) else {
                s.missing.push(a.ticker.clone());
                continue;
            };
            if p.online {
                if let Some(sym) = symbols.symbol_of(&a.ticker) {
                    s.books.push((a.clone(), sym));
                }
                if p.api_buy || p.api_sell {
                    s.tickers.push(a.ticker.clone());
                }
            } else {
                s.missing.push(a.ticker.clone());
            }
            s.pairs.push(p);
        }
        s
    }

    /// Событие ядру. Принимает: площадку. Возвращает: MarketEvent::SpotPairs.
    pub fn event(&self, venue: VenueId) -> MarketEvent {
        MarketEvent::SpotPairs {
            venue,
            tickers: self.tickers.clone(),
            pairs: self.pairs.clone(),
        }
    }

    /// Монеты, закрытые для API (status 1, но apiStateBuy и apiStateSell = false) — для журнала.
    pub fn api_closed(&self) -> Vec<String> {
        self.pairs
            .iter()
            .filter(|p| p.online && !p.api_buy && !p.api_sell)
            .map(|p| p.ticker.clone())
            .collect()
    }

    /// Чем этот снимок отличается от прежнего — строки «ТИКЕР: было → стало» (для журнала). Ошибок нет.
    pub fn changes(&self, old: &Snapshot) -> Vec<String> {
        let find = |s: &Snapshot, t: &str| s.pairs.iter().find(|p| p.ticker == t).cloned();
        let mut tickers: Vec<&str> = self
            .pairs
            .iter()
            .chain(old.pairs.iter())
            .map(|p| p.ticker.as_str())
            .collect();
        tickers.sort();
        tickers.dedup();
        tickers
            .into_iter()
            .filter_map(|t| {
                let (a, b) = (find(old, t), find(self, t));
                (a != b)
                    .then(|| format!("{t}: {} → {}", describe(a.as_ref()), describe(b.as_ref())))
            })
            .collect()
    }
}

/// Состояние пары коротко: «нет пары», «status N», «API: покупка да/нет, продажа да/нет». Ошибок нет.
fn describe(p: Option<&SpotPairState>) -> String {
    let yn = |b: bool| if b { "да" } else { "нет" };
    match p {
        None => "нет пары".into(),
        Some(p) if !p.online => format!("status {}", p.status),
        Some(p) => format!("API: покупка {}, продажа {}", yn(p.api_buy), yn(p.api_sell)),
    }
}
