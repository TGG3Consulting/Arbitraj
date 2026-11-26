//! Файл: core/convert_at.rs
//! Что делает: решение 211 (ревью В7) — цена валюты в USDT, когда известна площадка, где валюта возникла (удержание
//! вывода на бирже A, изменение баланса на площадке). Тикер у каждой биржи свой, и один тикер на двух биржах может
//! быть разными монетами — котировать «по имени» на чужой бирже нельзя:
//! - сначала площадка, где валюта возникла (её тикер — как она его дала);
//! - валюта — монета с адресом (адрес передан вызывающим — у DEX обязательно, тикер DEX лишь показное имя; иначе
//!   (площадка, тикер) → монета ядра, ticker_index.rs): на других биржах — по ИХ тикеру этого адреса; биржа, у которой
//!   этого адреса нет, пропускается;
//! - валюта без адреса (USDT, монеты venue.price_assets, монета вне справочника): только площадка возникновения;
//!   исключение — монета газа BNB (convert::GAS_TICKER, нативная монета сети одна у всех; с адресом — только нативная
//!   по native::is_native) — на всех биржах их тикером газа, как до решения 211.
//!
//! Котировка — convert::quote_one (средняя цена покупки нужного количества по живому стакану).
//! Кто вызывает: core/credit_event.rs (usd), core/precheck_withdraw.rs (held_usdt).
//! От чего зависит: convert.rs, venue_ticker.rs, ticker_index.rs, engine.rs.
//! Настройки: нет.

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};

use super::engine::{Core, rd};

/// Площадки и тикер на каждой — по порядку котирования. Принимает: ядро, биржи, площадку возникновения, тикер валюты
/// на ней, адрес валюты, если известен (второе ревью 211, п.7: у DEX тикер — показное имя, и у двух монет оно может
/// совпасть — монета ищется по адресу; без адреса — по тикеру площадки, ticker_index.rs). Возвращает: (биржа, тикер
/// этой биржи). Монета газа (нативная BNB — по тикеру газа без адреса или по тикеру и адресу обёртки, native.rs) —
/// на всех биржах их тикером газа, как до решения 211. Ошибок нет.
pub fn plan(
    core: &Core,
    cex: &[VenueId],
    origin: &VenueId,
    ticker: &str,
    addr: Option<Address>,
) -> Vec<(VenueId, String)> {
    let mut out = vec![];
    if cex.contains(origin) {
        out.push((origin.clone(), ticker.to_string()));
    }
    // Решение 416: тикер газа — dex.gas_ticker площадок DEX (не задана — валюта как любая другая). Решение 430: газ любой
    // сети площадок DEX (одна DEX — её тикер, как раньше).
    let dex_chain = if !cex.contains(origin) {
        match super::chain::tag_of_dex(core, origin) {
            Ok(chain) => Some(chain),
            Err(_) => return out,
        }
    } else {
        None
    };
    let idx = rd(&core.inner.tickers);
    let asset = match (addr, dex_chain) {
        (Some(address), Some(chain)) => Some(Asset {
            chain,
            contract: Some(address),
            ticker: ticker.to_owned(),
        }),
        (Some(address), None) => {
            let mut matching = idx
                .book_assets(origin, ticker)
                .filter(|a| a.contract == Some(address));
            match (matching.next(), matching.next()) {
                (Some(a), None) => Some(a),
                (None, None) => match idx.by_token.get(&address).map(Vec::as_slice) {
                    Some([unique]) => Some(unique.clone()),
                    None => Some(Asset {
                        chain: None,
                        contract: Some(address),
                        ticker: ticker.to_owned(),
                    }),
                    _ => None,
                },
                _ => None,
            }
        }
        (None, _) => {
            let mut matching = idx.book_assets(origin, ticker);
            matching.next().filter(|_| matching.next().is_none())
        }
    };
    let gas = super::gas_ticker::all(core).iter().any(|g| ticker == g)
        && (addr.is_none()
            || asset.as_ref().is_some_and(|a| {
                super::native::is_native(
                    core,
                    &Asset {
                        ticker: ticker.to_owned(),
                        ..a.clone()
                    },
                )
            }));
    for v in cex.iter().filter(|v| *v != origin) {
        if gas {
            out.push((v.clone(), ticker.to_string()));
            continue;
        }
        if let Some(t) = asset
            .as_ref()
            .and_then(|a| idx.venue_ticker.get(&(v.clone(), a.clone())))
        {
            out.push((v.clone(), t.clone()));
        }
    }
    out
}

/// Цена валюты в USDT (средняя цена покупки qty) по плану plan. Принимает: ядро, биржи (по порядку), площадку
/// возникновения, тикер валюты на ней, адрес валюты (если известен), количество. Ошибки: ни одна биржа плана не дала свежую котировку (причины);
/// план пуст — валюту без адреса не с чем котировать, кроме её площадки, а та не биржа.
pub async fn price_at(
    core: &Core,
    cex: &[VenueId],
    origin: &VenueId,
    ticker: &str,
    addr: Option<Address>,
    qty: Decimal,
) -> Result<Decimal, String> {
    let list = plan(core, cex, origin, ticker, addr);
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
            chain: None,
            ticker: t,
            contract: None,
        };
        match super::convert::quote_one(&core.inner.deps, &v, &a, qty).await {
            Ok(p) => return Ok(p),
            Err(e) => why.push(e),
        }
    }
    Err(format!("нет живой цены {ticker}: {}", why.join("; ")))
}
