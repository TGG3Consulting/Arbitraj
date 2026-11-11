//! Файл: report/snap_dex.rs
//! Что делает: решение 257 (доработка по приёмке t90, п. 3, 4) — части «Слепка по балансам», которых нет у бирж:
//! - кошельки DEX: что бот знает о кошельке — BNB, USDT (venue.quote_asset DEX; не задана — строка «нет данных» с
//!   причиной) и монеты своих сделок на кошельке (остаток Venue::balance по тикеру монеты); прочие токены кошелька бот
//!   не читает — строка «не учитывается»;
//!
//! «В пути» и сделки вне слепка — report/snap_transit.rs.
//!
//! Кто вызывает: report/snap.rs.
//! От чего зависит: report/snap.rs (Place, Line, loc, deal_line), report/snap_price.rs, core (dex_id, шлюз, настройки),
//! core/coin_key.rs, core/venue_ticker.rs, core/balance_sell_check.rs (quote_of), notify/human.rs.
//! Настройки: venue.quote_asset (читается).

use std::collections::BTreeMap;

use chrono::Utc;
use tokio::time::Instant;

use crate::core::Core;
use crate::core::balance_sell_check::quote_of;
use crate::core::deal::Deal;
use crate::core::types::{Asset, DexAccountId, VenueId};

use super::rows::f;
use super::snap::{Line, Loc, Place, deal_line, loc};
use super::snap_price::{balance, price};

/// Что слепок о кошельке не знает.
const OTHER_TOKENS: &str = "прочие токены кошелька не учитываются: бот читает с кошелька только BNB, USDT и монеты своих сделок";

/// Строка монеты: остаток чтением адаптера и цена котировкой (snap_price.rs). Ошибок нет: сбой — в строке.
async fn coin_line(
    core: &Core,
    gw: &std::sync::Arc<dyn crate::gateway::contract::Venue>,
    v: &VenueId,
    a: &Asset,
    quote: &Result<String, String>,
    note: &str,
    deadline: Instant,
) -> Line {
    let on = crate::core::venue_ticker::on_venue(core, a, v);
    let x = balance(gw, &on.ticker, deadline).await;
    let pr = match (&x, quote) {
        (Ok(x), Ok(qa)) => price(core, gw, v, a, *x, qa, deadline).await,
        (Err(e), _) => Err(format!("нет остатка: {e}")),
        (_, Err(e)) => Err(e.clone()),
    };
    let (price, n) = match pr {
        Ok((p, n)) => (Ok(p), format!("{note}; {n}")),
        Err(e) => (Err(e), note.to_string()),
    };
    Line {
        account: "кошелёк".into(),
        asset: a.ticker.clone(),
        qty: x.map(f),
        price,
        note: n,
        doubt: false,
    }
}

/// Кошельки DEX (см. шапку); сделки кошелька помечаются в `used`. Ошибок нет.
pub async fn wallets(
    core: &Core,
    held: &[&Deal],
    used: &mut [bool],
    deadline: Instant,
) -> Vec<Place> {
    let dex = core.dex_id();
    let mut ws = core.inner.deps.settings.accounts().wallets();
    ws.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = Vec::new();
    for w in ws {
        let acc = DexAccountId(w.id.clone());
        let mut p = Place {
            name: format!("Кошелёк DEX «{}»", w.name),
            unknown: vec![OTHER_TOKENS.into()],
            ..Place::default()
        };
        let mut coins: BTreeMap<String, Asset> = BTreeMap::new();
        for (k, d) in held.iter().enumerate() {
            let here = dex
                .as_ref()
                .is_some_and(|x| loc(d) == Loc::Venue(x.clone()));
            if here && d.dex_account.as_ref() == Some(&acc) {
                used[k] = true;
                p.deals.push(deal_line(d));
                let key = crate::core::coin_key::CoinKey::of(&d.direction.asset);
                coins.insert(format!("{key:?}"), d.direction.asset.clone());
            }
        }
        let Some(dv) = dex.clone() else {
            p.error = Some("площадка DEX не подключена — кошелёк не читается".into());
            out.push(p);
            continue;
        };
        let gw = match core.inner.deps.gw.get_as(&dv, Some(&acc)).await {
            Ok(g) => g,
            Err(e) => {
                p.error = Some(e.message);
                out.push(p);
                continue;
            }
        };
        p.at = Some(Utc::now());
        let quote = quote_of(core, &dv);
        let bnb = Asset {
            ticker: crate::core::convert::GAS_TICKER.into(),
            bsc_contract: None,
        };
        p.lines
            .push(coin_line(core, &gw, &dv, &bnb, &quote, "газ кошелька", deadline).await);
        match &quote {
            Ok(t) => {
                let x = balance(&gw, t, deadline).await;
                let note = "котируемая монета DEX (venue.quote_asset)".to_string();
                p.lines.push(Line {
                    account: "кошелёк".into(),
                    asset: t.clone(),
                    qty: x.map(f),
                    price: Ok(1.0),
                    note,
                    doubt: false,
                });
            }
            Err(e) => p.lines.push(Line {
                account: "кошелёк".into(),
                asset: "USDT (котируемая монета DEX)".into(),
                qty: Err(e.clone()),
                price: Err(e.clone()),
                note: String::new(),
                doubt: false,
            }),
        }
        for a in coins.values() {
            p.lines.push(
                coin_line(
                    core,
                    &gw,
                    &dv,
                    a,
                    &quote,
                    "монета сделки на кошельке",
                    deadline,
                )
                .await,
            );
        }
        for l in &p.lines {
            if let Err(e) = &l.qty {
                p.partial.push(format!("{}: {e}", l.asset));
            }
        }
        if p.lines.iter().all(|l| l.qty.is_err()) {
            p.error = Some(p.partial.join("; "));
        }
        out.push(p);
    }
    out
}
