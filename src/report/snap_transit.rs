//! Файл: report/snap_transit.rs
//! Что делает: решение 257 (доработка по приёмке t90, п. 3, 4) — части «Слепка по балансам» (вынесено из snap_dex.rs):
//! - «В пути» (все строки — «под вопросом»: ушёл ли вывод, по фактам сделки не определить — UNKNOWN; в итог не
//!   входят, отдельной суммой): сделки на шаге перевода (монеты по Deal.qty, цена — котировкой площадки продажи; при перегоне площадка
//!   назначения в учёте не записана — «нет цены») и части возврата USDT «идёт» (сумма части); «ждёт повтора» — только
//!   строкой (прежний вывод точно не ушёл — USDT в балансе площадки-отправителя);
//! - сделки, чья площадка не в реестре шлюза (выключена) или чей кошелёк не найден, — отдельный блок «нет данных».
//!
//! Кто вызывает: report/snap.rs.
//! От чего зависит: report/snap.rs (Place, Line, loc, deal_line), report/snap_price.rs, core (dex_id, шлюз),
//! core/usdt_return_parts.rs, core/balance_sell_check.rs (quote_of), notify/human.rs.
//! Настройки: venue.quote_asset (читается).

use std::collections::BTreeMap;

use chrono::Utc;
use tokio::time::Instant;

use crate::core::Core;
use crate::core::balance_sell_check::quote_of;
use crate::core::deal::Deal;
use crate::core::usdt_return_state::UsdtReturn;
use crate::notify::human::venue as name;

use super::rows::f;
use super::snap::{Line, Loc, Place, deal_line, loc};
use super::snap_price::price;

/// Почему строка «В пути» — под вопросом (факт кода): шаг «перевод» ставится до отправки вывода (core/executor.rs,
/// relay.rs, manual_relay.rs), а вывод записывается в сделку (Deal.transfers) только при зачислении (core/fills.rs
/// transfer); у части возврата «идёт» (UsdtReturn::Pending) id вывода нет.
const UNKNOWN: &str =
    "в учёте сделки нет признака, ушёл ли вывод: в итог не входит, сумма «под вопросом»";

/// «В пути» (см. шапку). Ошибок нет.
pub async fn transit(
    core: &Core,
    all: &[Deal],
    held: &[&Deal],
    used: &mut [bool],
    deadline: Instant,
) -> Place {
    let mut p = Place {
        name: "В пути".into(),
        at: Some(Utc::now()),
        ..Place::default()
    };
    for (k, d) in held
        .iter()
        .enumerate()
        .filter(|(_, d)| loc(d) == Loc::Transit)
    {
        used[k] = true;
        p.deals.push(deal_line(d));
        let from = name(&d.at.0);
        let (dest, pr) = if d.relays == 0 {
            let v = &d.direction.sell_on;
            let acc = d
                .dex_account
                .as_ref()
                .filter(|_| core.dex_id().as_ref() == Some(v));
            let pr = match core.inner.deps.gw.get_as(v, acc).await {
                Ok(gw) => match quote_of(core, v) {
                    Ok(qa) => price(core, &gw, v, &d.direction.asset, d.qty, &qa, deadline)
                        .await
                        .map(|x| x.0),
                    Err(e) => Err(e),
                },
                Err(e) => Err(e.message),
            };
            (name(&v.0), pr)
        } else {
            (
                "площадка перегона".to_string(),
                Err("площадка назначения перегона в учёте сделки не записана".into()),
            )
        };
        let note = format!(
            "сделка {}: шаг «перевод» от {from} к {dest} (цена — котировка {dest}); возможно, монеты ещё в балансе {from} — {UNKNOWN}",
            d.id
        );
        p.lines.push(Line {
            account: "перевод".into(),
            asset: d.direction.asset.ticker.clone(),
            qty: Ok(f(d.qty)),
            price: pr,
            note,
            doubt: true,
        });
    }
    for d in all {
        for part in crate::core::usdt_return_parts::all(d) {
            match part {
                UsdtReturn::Pending { amount, from, to } => p.lines.push(Line {
                    account: "возврат USDT".into(),
                    asset: "USDT".into(),
                    qty: Ok(f(*amount)),
                    price: Ok(1.0),
                    note: format!(
                        "сделка {}: возврат {} → {} идёт; возможно, сумма ещё в балансе {} — {UNKNOWN}",
                        d.id,
                        name(&from.0),
                        name(&to.0),
                        name(&from.0)
                    ),
                    doubt: true,
                }),
                UsdtReturn::Retrying { amount, from, .. } => p.deals.push(format!(
                    "Сделка {}: возврат USDT {} ждёт повтора — прежний вывод не ушёл, сумма в балансе {} (здесь не прибавляется)",
                    d.id, f(*amount), name(&from.0)
                )),
                _ => {}
            }
        }
    }
    p
}

/// Сделки, которых нет ни у одной прочитанной площадки (площадка не в реестре, кошелёк не найден). Ошибок нет.
pub fn orphans(held: &[&Deal], used: &[bool]) -> Vec<Place> {
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in held.iter().zip(used).filter(|(_, u)| !**u).map(|(d, _)| d) {
        let at = match loc(d) {
            Loc::Venue(v) => name(&v.0),
            Loc::Transit => "в пути".into(),
        };
        by.entry(at).or_default().push(deal_line(d));
    }
    by.into_iter()
        .map(|(n, deals)| Place {
            name: format!("{n} (вне слепка)"),
            error: Some("балансы не читаются: площадки нет в реестре шлюза (выключена) или кошелёк сделки не найден в настройках".into()),
            deals,
            ..Place::default()
        })
        .collect()
}
