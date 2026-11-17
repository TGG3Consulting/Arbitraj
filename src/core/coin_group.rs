//! Файл: core/coin_group.rs
//! Что делает: решение 390 (t179) — общий возврат USDT группы докупки (записка 269, пп. 10, 11, 13). Вызывается из
//! общего шага возврата (prepare в usdt_return_deal.rs) у закрытой сделки с группой (Deal.group; без группы — как раньше):
//! - сделка закрыта не обычной продажей на площадке плана (перегон, продажа здесь, вручную, части на нескольких
//!   площадках) — она вышла из группы (п. 13): свой возврат, пометка «была частью группы …»;
//! - идут другие сделки группы (шаг Transfer / Selling) — её выручка ждёт (п. 10): часть возврата откладывается в
//!   группу, у сделки возврат 0 с пометкой (deal_group.rs);
//! - она последняя — ОДИН возврат на общую сумму: своя часть + отложенные (п. 10); затраты возврата — один раз (п. 11,
//!   14) у первой сделки группы, если она среди отложенных (её уведомление deal.closed ждёт конца возврата, deal_closed
//!   hold), иначе у этой; у остальных — «общий возврат в сделке …»; итог группы — в уведомлении носителя (п. 12).
//!
//! Последняя идущая сделка группы ушла с пути (застряла, остановлена, ждёт человека) — её не ждём (п. 13): отложенные
//! части уходят одним возвратом сразу (on_saved), носитель — первая сделка группы или первая отложенная.
//! Решение «последняя ли» — под одним замком `groups` (две сделки, закрывающиеся одновременно, не отложат обе).
//! Сделки группы до учёта покупки (предпроверка) в «идут» не входят: последняя закрывшаяся из идущих возвращает сразу.
//! Отложенные части живут в памяти: перезапуск — ledger_load.rs ставит страховку «USDT не вернулись».
//! Кто вызывает: usdt_return_deal.rs (route), ledger.rs (save → on_saved).
//! От чего зависит: coin_key.rs, coin_ride.rs (идущие), deal.rs, deal_group.rs, ledger.rs, usdt_return_deal.rs, engine.rs.
//! Настройки: нет (включатель exec.top_up — coin_ride.rs).

use std::collections::HashSet;

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::deal_group::GroupSum;
use super::engine::{Core, lk};

/// Состояние группы: кто уже решал свой возврат и отложенные части (сделка, откуда, сумма).
#[derive(Debug, Default)]
pub struct Pool {
    closing: HashSet<String>,
    parts: Vec<(String, VenueId, Decimal)>,
}

/// Обычное закрытие сделки группы: продано ботом на площадке плана, без перегонов, одна площадка выручки, без
/// исключённого пути и «Продать и перегнать». Ошибок нет.
fn normal(deal: &Deal) -> bool {
    deal.at == deal.direction.sell_on
        && deal.relays == 0
        && !deal.sells.is_empty()
        && deal.usdt_return_skip.is_none()
        && !deal.return_usdt
}

/// Сколько других сделок группы `g` монеты ещё идут (Transfer / Selling) и не решали возврат. Вызывается под замком
/// `groups`. Ошибок нет.
fn running(core: &Core, key: &CoinKey, g: &str, me: &str, closing: &HashSet<String>) -> usize {
    let rides = lk(&core.inner.rides);
    let deals = lk(&core.inner.deals);
    rides.get(key).map_or(0, |v| {
        v.iter()
            .filter(|r| r.group.as_deref() == Some(g) && r.deal_id != me)
            .filter(|r| !closing.contains(&r.deal_id))
            .filter(|r| {
                matches!(
                    deals.get(&r.deal_id).map(|x| x.step),
                    Some(Step::Transfer | Step::Selling)
                )
            })
            .count()
    })
}

/// Решение о возврате закрытой сделки группы под замком `groups`.
enum Choice {
    /// Идут другие сделки группы — часть отложена; true — первая сделка группы уже среди отложенных (носитель — она).
    Defer(bool),
    /// Последняя, носитель — она сама: отложенные части других.
    Last(Vec<(String, VenueId, Decimal)>),
    /// Последняя, а первая сделка группы среди отложенных — носитель она (п. 11: возврат записывается на группу —
    /// сделку 1): все части, включая свою.
    ToLead(Vec<(String, VenueId, Decimal)>),
}

/// Что делает: решает возврат закрытой сделки группы (см. шапку). Принимает: ядро, сделку (запишутся пометки и
/// Deal.group), её части возврата (откуда, сумма). Возвращает: части, которые перегнать сейчас этой сделкой (пусто —
/// отложено или перегоняет первая сделка группы). Ошибок нет.
pub async fn route(
    core: &Core,
    deal: &mut Deal,
    parts: Vec<(VenueId, Decimal)>,
) -> Vec<(VenueId, Decimal)> {
    let Some(g) = deal.group.as_ref().map(|x| x.id.clone()) else {
        return parts;
    };
    if !normal(deal) || parts.len() != 1 {
        if let Some(x) = deal.group.as_mut() {
            x.left = true;
        }
        deal.note(format!(
            "Была частью группы докупки {g}: закрыта не обычной продажей на площадке плана — свой возврат USDT (решение 390)"
        ));
        return parts;
    }
    let key = CoinKey::of(&deal.direction.asset);
    let (from, amount) = parts[0].clone();
    let me = (deal.id.clone(), from.clone(), amount);
    let choice = {
        let mut groups = lk(&core.inner.groups);
        let pool = groups.entry(g.clone()).or_default();
        pool.closing.insert(deal.id.clone());
        if running(core, &key, &g, &deal.id, &pool.closing) > 0 {
            pool.parts.push(me);
            Choice::Defer(pool.parts.iter().any(|(id, _, _)| *id == g))
        } else {
            let mut all = groups.remove(&g).map(|p| p.parts).unwrap_or_default();
            if deal.id != g && all.iter().any(|(id, _, _)| *id == g) {
                all.push(me);
                Choice::ToLead(all)
            } else {
                Choice::Last(all)
            }
        }
    };
    match choice {
        Choice::Defer(lead) => {
            if let Some(x) = deal.group.as_mut() {
                x.wait = Some((from.clone(), amount));
                if lead && deal.id != g {
                    x.shared_in = Some(g.clone());
                }
            }
            let tail = if lead && deal.id != g {
                format!("общий возврат группы в сделке {g}")
            } else {
                format!("общий возврат группы {g} после последней её сделки")
            };
            deal.note(format!(
                "Возврат USDT {amount} с {}: 0 сейчас — {tail} (решение 390)",
                from.0
            ));
            vec![]
        }
        Choice::ToLead(all) => {
            if let Some(x) = deal.group.as_mut() {
                x.wait = None;
                x.shared_in = Some(g.clone());
            }
            deal.note(format!(
                "Возврат USDT {amount}: 0 — общий возврат группы в сделке {g} (решение 390)"
            ));
            let mine = (deal.id.clone(), deal.result_usd);
            let c = core.clone();
            tokio::spawn(async move { flush(&c, &g, all, Some(mine)).await });
            vec![]
        }
        Choice::Last(others) => {
            let total = amount + others.iter().map(|(_, _, a)| *a).sum::<Decimal>();
            set_sum(core, deal, &others, None);
            if !others.is_empty() {
                deal.note(format!(
                    "Общий возврат USDT группы {g}: {total} (своя часть {amount} + отложенные сделок {}) — затраты возврата один раз, в этой сделке (решение 390)",
                    ids(&others).join(", ")
                ));
                mark_shared(core, &others, &deal.id, None).await;
            }
            vec![(from, total)]
        }
    }
}

/// ID сделок отложенных частей. Ошибок нет.
fn ids(parts: &[(String, VenueId, Decimal)]) -> Vec<String> {
    parts.iter().map(|(id, _, _)| id.clone()).collect()
}

/// Итог группы у сделки с общим возвратом: другие сделки и сумма их итогов (из учёта; `known` — итог сделки, ещё не
/// сохранённой в учёт). Ошибок нет.
fn set_sum(
    core: &Core,
    deal: &mut Deal,
    others: &[(String, VenueId, Decimal)],
    known: Option<&(String, Option<Decimal>)>,
) {
    if others.is_empty() {
        return;
    }
    let sum: Decimal = {
        let deals = lk(&core.inner.deals);
        others
            .iter()
            .filter_map(|(id, _, _)| match known {
                Some((k, r)) if k == id => *r,
                _ => deals.get(id).and_then(|x| x.result_usd),
            })
            .sum()
    };
    let usdt = others.iter().map(|(_, _, a)| *a).sum();
    if let Some(x) = deal.group.as_mut() {
        x.wait = None;
        x.sum = Some(GroupSum {
            deals: ids(others),
            others: sum,
            usdt,
        });
    }
}

/// Отложенные сделки: общий возврат — в сделке `bearer` (возврат 0, пометка). `skip` — сделка, которую сохранит её
/// собственное закрытие (не перезаписывать её запись старой копией). Принимает: ядро, части, носителя, skip.
/// Ошибок нет.
async fn mark_shared(
    core: &Core,
    parts: &[(String, VenueId, Decimal)],
    bearer: &str,
    skip: Option<&str>,
) {
    for (id, _, _) in parts {
        if Some(id.as_str()) == skip {
            continue;
        }
        let d = lk(&core.inner.deals).get(id).cloned();
        let Some(mut d) = d else { continue };
        if let Some(x) = d.group.as_mut() {
            if x.shared_in.as_deref() == Some(bearer) && x.wait.is_none() {
                continue;
            }
            x.wait = None;
            x.shared_in = Some(bearer.to_string());
        }
        d.note(format!(
            "Возврат USDT: 0 — общий возврат группы в сделке {bearer} (решение 390)"
        ));
        super::ledger::save(core, &d).await;
    }
}

/// Что делает: сделка группы сохранена на шаге вне пути (застряла, ждёт человека, остановлена, передана человеку) —
/// если других идущих сделок группы нет, а отложенные части есть, они уходят одним возвратом сразу (п. 13), носитель —
/// первая отложенная сделка. Принимает: ядро, сделку. Ошибок нет (задача возврата — отдельной задачей).
pub fn on_saved(core: &Core, deal: &Deal) {
    let Some(g) = deal.group.as_ref().map(|x| x.id.clone()) else {
        return;
    };
    if matches!(
        deal.step,
        Step::Buying | Step::Transfer | Step::Selling | Step::Closed
    ) {
        return;
    }
    let key = CoinKey::of(&deal.direction.asset);
    let parts = {
        let mut groups = lk(&core.inner.groups);
        let Some(pool) = groups.get(&g) else { return };
        if pool.parts.is_empty() || running(core, &key, &g, &deal.id, &pool.closing) > 0 {
            return;
        }
        groups.remove(&g).map(|p| p.parts).unwrap_or_default()
    };
    let c = core.clone();
    tokio::spawn(async move { flush(&c, &g, parts, None).await });
}

/// Общий возврат отложенных частей одним перегоном. Носитель — первая сделка группы, если она среди частей (п. 11),
/// иначе первая отложенная; её отложенное уведомление deal.closed (deal_closed::hold) уйдёт по концу перегона с итогом
/// группы. Принимает: ядро, группу, части, итог сделки, ещё не сохранённой в учёт (None — все в учёте). Ошибок нет.
async fn flush(
    core: &Core,
    g: &str,
    parts: Vec<(String, VenueId, Decimal)>,
    known: Option<(String, Option<Decimal>)>,
) {
    let Some(first) = parts.first().cloned() else {
        return;
    };
    let (bearer, from, _) = parts
        .iter()
        .find(|(id, _, _)| id == g)
        .cloned()
        .unwrap_or(first);
    let d = lk(&core.inner.deals).get(&bearer).cloned();
    let Some(mut d) = d else {
        core.inner
            .deps
            .log(
                Level::Error,
                "core.usdt_return",
                Some(&from.0),
                format!("Группа {g}: сделки {bearer} нет в учёте — общий возврат USDT не начат"),
                None,
            )
            .await;
        return;
    };
    let total: Decimal = parts.iter().map(|(_, _, a)| *a).sum();
    let others: Vec<_> = parts
        .iter()
        .filter(|(id, _, _)| *id != bearer)
        .cloned()
        .collect();
    set_sum(core, &mut d, &others, known.as_ref());
    if let Some(x) = d.group.as_mut() {
        x.wait = None;
        x.shared_in = None;
    }
    let why = if known.is_some() {
        "последняя сделка группы продана"
    } else {
        "последняя идущая сделка группы ушла с пути, её не ждём"
    };
    d.note(format!(
        "Общий возврат USDT группы {g}: {total} — {why}; затраты возврата один раз, в этой сделке (решение 390)"
    ));
    let jobs = super::usdt_return_deal::start_parts(core, &mut d, vec![(from, total)]).await;
    // Перегон не начат (страховка «не вернулись») — отложенное уведомление носителя уходит сразу.
    let notice = if jobs.is_empty() {
        super::deal_closed::take_ready(&mut d)
    } else {
        None
    };
    super::ledger::save(core, &d).await;
    if let Some(w) = notice {
        super::deal_closed::send(core, &d, &w).await;
    }
    for j in jobs {
        j.start(core);
    }
    let skip = known.as_ref().map(|(id, _)| id.as_str());
    mark_shared(core, &others, &bearer, skip).await;
}
