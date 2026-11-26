//! Слепок для ежедневной выгрузки по последним ответам панели и учёту переводов.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use super::{
    rows::f,
    snap::{Line, Loc, Place, Snapshot, deal_line, holds, loc},
    snap_transit,
};
use crate::core::{Core, balance_sell_check::quote_of, deal::Deal, types::VenueId};
use crate::notify::human::venue;

fn measurement(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    format!(
        "последний ответ панели {} UTC, возраст {} с; цена иных монет в панели не хранится",
        at.format("%Y-%m-%d %H:%M:%S"),
        (now - at).num_seconds().max(0)
    )
}

fn line(account: &str, asset: &str, qty: Option<Decimal>, quote: Option<&str>) -> Line {
    Line {
        account: account.into(),
        asset: asset.into(),
        qty: qty
            .map(f)
            .ok_or_else(|| "нет количества в ответе панели".into()),
        price: if Some(asset) == quote {
            Ok(1.0)
        } else {
            Err("нет локальной оценки в USDT".into())
        },
        note: "последний ответ панели балансов".into(),
        doubt: false,
    }
}

fn cex(core: &Core, v: &VenueId, now: DateTime<Utc>) -> Place {
    let mut p = Place {
        name: venue(&v.0),
        ..Default::default()
    };
    let Some(view) = core.cached_venue_balances(v) else {
        p.error = Some("нет ответа в памяти панели балансов".into());
        return p;
    };
    p.at = Some(view.at);
    p.deals.push(measurement(view.at, now));
    p.error = view.error;
    let quote = quote_of(core, v).ok();
    for a in view.accounts {
        if let Some(error) = a.error {
            p.partial.push(format!("счёт {}: {error}", a.account));
        }
        for r in a.rows {
            let qty = r.free + r.locked;
            if !qty.is_zero() {
                p.lines
                    .push(line(&a.account, &r.asset, Some(qty), quote.as_deref()));
            }
        }
    }
    p
}

fn wallet(core: &Core, v: &VenueId, id: &str, name: &str, now: DateTime<Utc>) -> Place {
    let mut p = Place {
        name: format!("Кошелёк DEX «{name}», {}", v.0),
        ..Default::default()
    };
    let Some(view) = core.cached_wallet_balances(v, id) else {
        p.error = Some("нет ответа кошелька в памяти панели балансов".into());
        return p;
    };
    p.at = Some(view.at);
    p.deals.push(measurement(view.at, now));
    if let Some(e) = view.error {
        p.partial.push(e);
    }
    let gas = crate::core::gas_ticker::at(&core.inner.deps.settings, v);
    match gas {
        Ok(g) => p
            .lines
            .push(line("кошелёк", &g, view.bnb, view.quote.as_deref())),
        Err(e) => p.partial.push(e),
    }
    if let Some(q) = view.quote {
        p.lines.push(line("кошелёк", &q, view.usdt, Some(&q)));
    }
    p.partial
        .push("панель хранит остатки газа и котируемой монеты".into());
    p
}

/// У обеих книг один слепок с фактическим временем каждого измерения. Никаких сетевых запросов.
pub(crate) async fn gather(core: &Core, deals: &[Deal], now: DateTime<Utc>) -> Snapshot {
    let held: Vec<_> = deals.iter().filter(|d| holds(d)).collect();
    let mut used = vec![false; held.len()];
    let mut places = vec![];
    let mut attach = |place: &mut Place, v: &VenueId, wallet: Option<&str>| {
        for (i, deal) in held.iter().enumerate() {
            if loc(deal) == Loc::Venue(v.clone())
                && wallet.is_none_or(|id| deal.dex_account.as_ref().is_some_and(|a| a.0 == id))
            {
                used[i] = true;
                place.deals.push(deal_line(deal));
            }
        }
    };
    let mut cex_ids = core.cex_ids();
    cex_ids.sort_by(|a, b| a.0.cmp(&b.0));
    for v in cex_ids {
        let mut p = cex(core, &v, now);
        attach(&mut p, &v, None);
        places.push(p);
    }
    let dexes = crate::core::chain::dexes(core);
    for w in core.inner.deps.settings.accounts().wallets() {
        if dexes.is_empty() {
            places.push(Place {
                name: format!("Кошелёк DEX «{}»", w.name),
                error: Some("нет подключённой площадки DEX".into()),
                ..Default::default()
            });
        }
        for v in &dexes {
            let mut p = wallet(core, v, &w.id, &w.name, now);
            attach(&mut p, v, Some(&w.id));
            places.push(p);
        }
    }
    // Нулевой остаток возможен в полученном полном ответе биржи. Пустой кеш не является таким ответом.
    let measured = places.iter().any(|p| {
        p.at.is_some()
            && p.error.is_none()
            && ((p.lines.is_empty() && p.partial.is_empty())
                || p.lines.iter().any(|l| l.qty.is_ok() && l.price.is_ok()))
    });
    places.push(snap_transit::transit_with_price(core, deals, &held, &mut used, None).await);
    places.extend(snap_transit::orphans(&held, &used));
    let problem = (!measured)
        .then(|| "в памяти панели нет измерения с известной стоимостью в USDT".to_string());
    Snapshot {
        cached: true,
        places,
        problem,
    }
}
