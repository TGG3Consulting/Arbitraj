//! Файл: gateway/aliased_tr.rs
//! Что делает: решение 200 — перевод тикеров склеенной монеты в данных ядра (gateway/alias_map.rs VenueMap): в запросах
//! к бирже основной тикер → тикер биржи (out_*), в ответах и событиях биржи тикер биржи → основной (back_*). Только
//! поле тикера (Asset.ticker, Balance.asset, Cost.asset, fee_asset, тикеры SpotPairs и Invalidated); адреса, числа и
//! всё остальное — как есть. Монеты вне карты не меняются.
//! Кто вызывает: gateway/aliased.rs.
//! От чего зависит: gateway/alias_map.rs, gateway/events.rs, core/types.
//! Настройки: нет.

use crate::core::types::{
    AccountBalances, Asset, Balance, CoinNetworkInfo, Cost, Quote, TradeRequest, TradeResult,
    Transfer, WithdrawRequest,
};

use super::alias_map::VenueMap;
use super::events::MarketEvent;

/// Монета для запроса к бирже. Ошибок нет.
pub fn out_asset(m: &VenueMap, a: &Asset) -> Asset {
    match m.out(&a.ticker, a.bsc_contract) {
        Some(t) => Asset {
            ticker: t.to_string(),
            bsc_contract: a.bsc_contract,
        },
        None => a.clone(),
    }
}

/// Тикер строкой для запроса к бирже (адреса нет). Ошибок нет.
pub fn out_str(m: &VenueMap, t: &str) -> String {
    m.out(t, None).unwrap_or(t).to_string()
}

/// Тикер биржи → основной. Ошибок нет.
pub fn back_str(m: &VenueMap, t: &str) -> String {
    m.back(t).unwrap_or(t).to_string()
}

/// Монета из ответа биржи. Ошибок нет.
pub fn back_asset(m: &VenueMap, mut a: Asset) -> Asset {
    a.ticker = back_str(m, &a.ticker);
    a
}

/// Затраты: монета затраты (комиссия в монете сделки). Ошибок нет.
pub fn back_costs(m: &VenueMap, v: Vec<Cost>) -> Vec<Cost> {
    v.into_iter()
        .map(|mut c| {
            c.asset = back_str(m, &c.asset);
            c
        })
        .collect()
}

/// Запрос ордера. Ошибок нет.
pub fn out_trade(m: &VenueMap, mut r: TradeRequest) -> TradeRequest {
    r.asset = out_asset(m, &r.asset);
    r
}

/// Запрос вывода. Ошибок нет.
pub fn out_withdraw(m: &VenueMap, mut r: WithdrawRequest) -> WithdrawRequest {
    r.asset = out_asset(m, &r.asset);
    r
}

/// Котировка. Ошибок нет.
pub fn back_quote(m: &VenueMap, mut q: Quote) -> Quote {
    q.asset = back_asset(m, q.asset);
    q.costs = back_costs(m, q.costs);
    q
}

/// Итог ордера. Ошибок нет.
pub fn back_trade(m: &VenueMap, mut r: TradeResult) -> TradeResult {
    r.request.asset = back_asset(m, r.request.asset);
    r.fee_asset = back_str(m, &r.fee_asset);
    r
}

/// Перевод. Ошибок нет.
pub fn back_transfer(m: &VenueMap, mut t: Transfer) -> Transfer {
    t.asset = back_asset(m, t.asset);
    t.fee = t.fee.map(|c| back_costs(m, vec![c]).remove(0));
    t
}

/// Баланс. Ошибок нет.
pub fn back_balance(m: &VenueMap, mut b: Balance) -> Balance {
    b.asset = back_str(m, &b.asset);
    b
}

/// Балансы по счетам. Ошибок нет.
pub fn back_accounts(m: &VenueMap, v: Vec<AccountBalances>) -> Vec<AccountBalances> {
    v.into_iter()
        .map(|mut a| {
            a.balances = a.balances.into_iter().map(|b| back_balance(m, b)).collect();
            a
        })
        .collect()
}

/// Справочник сетей биржи: все сети монеты с тикером биржи — под основным тикером. Ошибок нет.
pub fn back_networks(m: &VenueMap, v: Vec<CoinNetworkInfo>) -> Vec<CoinNetworkInfo> {
    v.into_iter()
        .map(|mut i| {
            i.asset = back_asset(m, i.asset);
            i
        })
        .collect()
}

/// Событие рынка биржи. Ошибок нет.
pub fn back_event(m: &VenueMap, e: MarketEvent) -> MarketEvent {
    match e {
        MarketEvent::Book(mut b) => {
            b.asset = back_asset(m, b.asset);
            MarketEvent::Book(b)
        }
        MarketEvent::Invalidated { venue, ticker } => MarketEvent::Invalidated {
            venue,
            ticker: back_str(m, &ticker),
        },
        MarketEvent::SpotPairs {
            venue,
            tickers,
            mut pairs,
            loaded_at,
        } => {
            for p in &mut pairs {
                p.ticker = back_str(m, &p.ticker);
            }
            MarketEvent::SpotPairs {
                venue,
                tickers: tickers.iter().map(|t| back_str(m, t)).collect(),
                pairs,
                loaded_at,
            }
        }
        MarketEvent::Balance(mut b) => {
            b.asset = back_asset(m, b.asset);
            MarketEvent::Balance(b)
        }
        other => other,
    }
}
