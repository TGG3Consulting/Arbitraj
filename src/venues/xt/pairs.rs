//! Файл: venues/xt/pairs.rs
//! Что делает: снимок пар монет подписки по справочнику пар (symbols.rs): на какие стаканы подписываться,
//! каких пар нет, какие монеты торгуются (для подписки DEX в ядре) и состояние каждой пары (статус, покупка и
//! продажа через API) — событие ядру MarketEvent::SpotPairs.
//! Пара, на которой XT не примет наш ордер (тип LIMIT и timeInForce по справочнику — решение 254), помечается
//! причиной (SpotPairState::order_block): ядро не берёт её в расчёт ни для покупки, ни для продажи.
//! Кто вызывает: market.rs (при подписке), pairs_watch.rs (при обновлении справочника пар).
//! От чего зависит: symbols.rs, order_rules.rs (через symbols.rs), sign.rs (value_ok), gateway/events, core/types.
//! Настройки: venue.order_time_in_force — значение передают market.rs и pairs_watch.rs.
//!
//! Официально: GET /v4/public/symbol (state — перечень «Symbol state» publicModule.md, tradingEnabled, openapiEnabled)
//! https://doc.xt.com/docs/spot/Market/GetSymbolInformation (`_market/2symbol.md`).

use chrono::{DateTime, Utc};

use crate::core::types::{Asset, VenueId};
use crate::gateway::events::{MarketEvent, SpotPairState};

use super::symbols::Symbols;

/// Снимок пар монет подписки.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Стаканы для подписки: монета и символ XT — пары в состоянии ONLINE. Стакан нужен
    /// и паре, закрытой для API: его цена — живая цена монеты (газ, промежуточные монеты, core/convert.rs).
    pub books: Vec<(Asset, String)>,
    /// Монеты без пары в состоянии ONLINE (для журнала).
    pub missing: Vec<String>,
    /// Символы пар ONLINE со знаками, которые нельзя поставить в запрос XT (sign::value_ok; 06.11.2026 — «牛来_usdt»,
    /// «龙虾_usdt»): снимок стакана REST и ордер для них не отправить, поэтому стакан не подписывается и через API
    /// монета не торгуется (для журнала — один раз при подписке, вместо отказа снимка каждую секунду).
    pub bad_symbols: Vec<String>,
    /// Монеты, которыми можно торговать через API хотя бы в одну сторону (онлайн и разрешена ключу в одну из сторон).
    pub tickers: Vec<String>,
    /// Состояние пары каждой монеты, у которой пара есть в справочнике.
    pub pairs: Vec<SpotPairState>,
}

impl Snapshot {
    /// Строит снимок по справочнику пар. Принимает: справочник, монеты подписки, timeInForce из настройки
    /// venue.order_time_in_force (None — не задан). Ошибок нет: монета без пары попадает в missing и в pairs не входит.
    /// Пара, на которой XT не примет наш ордер (решение 254, order_block), остаётся в pairs с причиной (ядро её не
    /// считает), стакан подписывается (живая цена монеты), в торгуемые (tickers) не входит.
    pub fn build(symbols: &Symbols, assets: &[Asset], tif: Option<&str>) -> Self {
        let mut s = Snapshot {
            books: vec![],
            missing: vec![],
            bad_symbols: vec![],
            tickers: vec![],
            pairs: vec![],
        };
        for a in assets {
            let Some(mut p) = symbols.state(&a.ticker, tif) else {
                s.missing.push(a.ticker.clone());
                continue;
            };
            if p.online {
                let sym = symbols.symbol_of(&a.ticker);
                if let Some(bad) = sym.as_ref().filter(|x| !super::sign::value_ok(x)) {
                    s.bad_symbols.push(bad.clone());
                    (p.api_buy, p.api_sell) = (false, false);
                    s.pairs.push(p);
                    continue;
                }
                if let Some(sym) = sym {
                    s.books.push((a.clone(), sym));
                }
                if (p.api_buy || p.api_sell) && p.order_block.is_none() {
                    s.tickers.push(a.ticker.clone());
                }
            } else {
                s.missing.push(a.ticker.clone());
            }
            s.pairs.push(p);
        }
        s
    }

    /// Событие ядру. Принимает: площадку, когда справочник пар последний раз успешно получен (Symbols::loaded_at).
    /// Возвращает: MarketEvent::SpotPairs.
    pub fn event(&self, venue: VenueId, loaded_at: DateTime<Utc>) -> MarketEvent {
        MarketEvent::SpotPairs {
            venue,
            tickers: self.tickers.clone(),
            pairs: self.pairs.clone(),
            loaded_at,
        }
    }

    /// Монеты, закрытые для API (ONLINE, но tradingEnabled или openapiEnabled = false) — для журнала.
    pub fn api_closed(&self) -> Vec<String> {
        self.pairs
            .iter()
            .filter(|p| p.online && !p.api_buy && !p.api_sell)
            .map(|p| p.ticker.clone())
            .collect()
    }

    /// Пары, на которых XT не примет наш ордер (решение 254) — причины для журнала. Ошибок нет.
    pub fn order_blocked(&self) -> Vec<String> {
        self.pairs
            .iter()
            .filter(|p| p.online)
            .filter_map(|p| p.order_block.clone())
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

/// Состояние пары коротко: «нет пары», «state ИМЯ», «не примет наш ордер (причина)», «API: покупка да/нет, продажа
/// да/нет». Ошибок нет.
fn describe(p: Option<&SpotPairState>) -> String {
    let yn = |b: bool| if b { "да" } else { "нет" };
    match p {
        None => "нет пары".into(),
        Some(p) if !p.online => format!("state {}", super::symbols::state_name(p.status)),
        Some(p) if p.order_block.is_some() => format!(
            "не примет наш ордер ({})",
            p.order_block.clone().unwrap_or_default()
        ),
        Some(p) => format!("API: покупка {}, продажа {}", yn(p.api_buy), yn(p.api_sell)),
    }
}
