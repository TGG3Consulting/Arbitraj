//! Файл: report/snap_cex.rs
//! Что делает: решение 257 — блок биржи «Слепка по балансам» (вынесено из snap.rs): все счета чтением адаптера
//! (Venue::account_balances; память панели «Балансы» не трогается), цена каждой монеты — report/snap_price.rs
//! (котировка площадки, только чтение); котируемая монета (venue.quote_asset) = 1; счёт не прочитан — «неполно».
//! Кто вызывает: report/snap.rs.
//! От чего зависит: core (шлюз, настройки), core/balance_sell_check.rs (quote_of, asset_of), report/snap_price.rs.
//! Настройки: venue.quote_asset (читается).

use chrono::Utc;
use tokio::time::Instant;

use crate::core::Core;
use crate::core::balance_sell_check::{asset_of, quote_of};
use crate::core::types::VenueId;
use crate::notify::human::venue as name;

use super::rows::f;
use super::snap::{Line, Place};
use super::snap_price::{price, until};

/// Биржа: все счета, цена каждой монеты. Ошибок нет: сбой — в площадке.
pub async fn place(core: &Core, v: &VenueId, deadline: Instant) -> Place {
    let mut p = Place {
        name: name(&v.0),
        ..Place::default()
    };
    let gw = match core.inner.deps.gw.get(v).await {
        Ok(g) => g,
        Err(e) => {
            p.error = Some(e.message);
            return p;
        }
    };
    let list = match until(deadline, gw.account_balances())
        .await
        .and_then(|r| r.map_err(|e| e.message))
    {
        Ok(l) => l,
        Err(e) => {
            p.error = Some(e);
            return p;
        }
    };
    p.at = Some(Utc::now());
    let quote = quote_of(core, v);
    for a in list {
        if let Some(e) = &a.error {
            p.partial
                .push(format!("счёт «{}» не прочитан: {e}", a.account));
        }
        for r in a.balances.iter().filter(|r| !(r.free + r.locked).is_zero()) {
            let x = r.free + r.locked;
            let (pr, note) = match &quote {
                Ok(qa) if qa == &r.asset => (
                    Ok(1.0),
                    "котируемая монета площадки (venue.quote_asset)".to_string(),
                ),
                Ok(qa) => {
                    match price(core, &gw, v, &asset_of(core, v, &r.asset), x, qa, deadline).await {
                        Ok((p, n)) => (Ok(p), n),
                        Err(e) => (Err(e), String::new()),
                    }
                }
                Err(e) => (Err(e.clone()), String::new()),
            };
            let line = Line {
                account: a.account.clone(),
                asset: r.asset.clone(),
                qty: Ok(f(x)),
                price: pr,
                note,
            };
            p.lines.push(line);
        }
    }
    p
}
