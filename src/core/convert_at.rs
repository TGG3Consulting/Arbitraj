//! Файл: core/convert_at.rs
//! Что делает: решение 211 (ревью В7) — цена валюты в USDT, когда известна площадка, где валюта возникла (удержание
//! вывода на бирже A, изменение баланса на площадке). Тикер у каждой биржи свой, и один тикер на двух биржах может
//! быть разными монетами — котировать «по имени» на чужой бирже нельзя:
//! - сначала площадка, где валюта возникла (её тикер — как она его дала);
//! - валюта — монета с адресом ((площадка, тикер) → монета ядра, ticker_index.rs): на других биржах — по ИХ тикеру
//!   этого адреса; биржа, у которой этого адреса нет, пропускается;
//! - валюта без адреса (USDT, монеты venue.price_assets, монета вне справочника): только площадка возникновения;
//!   исключение — монета газа BNB (convert::GAS_TICKER, нативная монета сети одна у всех) — на всех биржах, как раньше.
//!
//! Котировка — convert::quote_one (средняя цена покупки нужного количества по живому стакану).
//! Кто вызывает: core/credit_event.rs (usd), core/precheck_withdraw.rs (held_usdt).
//! От чего зависит: convert.rs, venue_ticker.rs, ticker_index.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};

use super::engine::{Core, rd};

/// Площадки и тикер на каждой — по порядку котирования. Принимает: ядро, биржи, площадку возникновения, тикер валюты
/// на ней. Возвращает: (биржа, тикер этой биржи). Ошибок нет.
pub fn plan(
    core: &Core,
    cex: &[VenueId],
    origin: &VenueId,
    ticker: &str,
) -> Vec<(VenueId, String)> {
    let mut out = vec![];
    if cex.contains(origin) {
        out.push((origin.clone(), ticker.to_string()));
    }
    let addr = super::venue_ticker::asset_by_venue_ticker(core, origin, ticker)
        .and_then(|a| a.bsc_contract);
    let idx = rd(&core.inner.tickers);
    for v in cex.iter().filter(|v| *v != origin) {
        match addr {
            Some(a) => {
                if let Some(t) = idx.venue_ticker.get(&(v.clone(), a)) {
                    out.push((v.clone(), t.clone()));
                }
            }
            None if ticker == super::convert::GAS_TICKER => {
                out.push((v.clone(), ticker.to_string()))
            }
            None => {}
        }
    }
    out
}

/// Цена валюты в USDT (средняя цена покупки qty) по плану plan. Принимает: ядро, биржи (по порядку), площадку
/// возникновения, тикер валюты на ней, количество. Ошибки: ни одна биржа плана не дала свежую котировку (причины);
/// план пуст — валюту без адреса не с чем котировать, кроме её площадки, а та не биржа.
pub async fn price_at(
    core: &Core,
    cex: &[VenueId],
    origin: &VenueId,
    ticker: &str,
    qty: Decimal,
) -> Result<Decimal, String> {
    let list = plan(core, cex, origin, ticker);
    if list.is_empty() {
        return Err(format!(
            "нет живой цены {ticker}: котировать можно только там, где валюта возникла ({}), а это не биржа, или монеты \
             с этим адресом нет на других биржах",
            origin.0
        ));
    }
    let mut why = vec![];
    for (v, t) in list {
        let a = Asset {
            ticker: t,
            bsc_contract: None,
        };
        match super::convert::quote_one(&core.inner.deps, &v, &a, qty).await {
            Ok(p) => return Ok(p),
            Err(e) => why.push(e),
        }
    }
    Err(format!("нет живой цены {ticker}: {}", why.join("; ")))
}
