//! Файл: tests/t489_common/deals.rs
//! Что делает: решение 489 — сделки тестов через настоящий arb::core::ledger::save (учёт памяти, unsettled, SQLite ядра
//! t198_common, feed_hold::on_saved и net_hold::on_saved — тот же путь, что у бота): идущая сделка bingx → площадка
//! DEX (решение 196: держит площадку), её правка и сохранение, части возврата USDT («идёт», «ждёт повтора»,
//! «вернулись», «не вернулись»), ожидание общего возврата группы; память ядра «сеть держат сделки» (Inner.net_held);
//! remember — учёт памяти без сохранения (для проверки возвращаемого значения net_hold::on_saved).
//! Кто вызывает: tests/core_net_chain_489_held.rs, tests/core_net_chain_489_bsc.rs, tests/core_net_chain_489_recap.rs,
//! tests/core_net_chain_489_flush.rs.
//! От чего зависит: arb::core (deal, deal_group, ledger, unsettled, usdt_return_state). Моков нет.

#![allow(dead_code)]

use rust_decimal::Decimal;
use serde_json::json;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deal_group::DealGroup;
use arb::core::types::{Asset, ChainId, Direction, VenueId};
use arb::core::usdt_return_state::UsdtReturn;

/// Площадка по id. Ошибок нет.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Новая сделка bingx → `dex` монеты сети `chain` (шаг «покупка» — открыта). Ошибок нет.
pub fn new_deal(id: &str, dex: &str, chain: Option<ChainId>) -> Deal {
    let dir = Direction {
        asset: Asset {
            chain,
            ticker: "USDT".into(),
            contract: None,
        },
        buy_on: v("bingx"),
        sell_on: v(dex),
    };
    Deal::new(id.into(), dir, json!({}), Decimal::ZERO)
}

/// Идущая сделка bingx → `dex` (держит площадку по решению 196), сохранена ledger::save. Ошибок нет.
pub async fn hold(core: &Core, id: &str, dex: &str, chain: Option<ChainId>) {
    arb::core::ledger::save(core, &new_deal(id, dex, chain)).await;
}

/// Сделка — в учёт памяти и набор незавершённых (как первые шаги ledger::save: учёт, unsettled::track), без записи в
/// базу и без net_hold::on_saved — его тест зовёт сам и проверяет возвращаемое значение. Ошибок нет.
pub fn remember(core: &Core, d: &Deal) {
    core.inner
        .deals
        .lock()
        .unwrap()
        .insert(d.id.clone(), d.clone());
    arb::core::unsettled::track(core, d);
}

/// Правка сделки `id` из учёта и сохранение ledger::save. Паника — сделки нет.
pub async fn edit(core: &Core, id: &str, f: impl FnOnce(&mut Deal)) {
    let mut d = core.inner.deals.lock().unwrap().get(id).cloned().unwrap();
    f(&mut d);
    arb::core::ledger::save(core, &d).await;
}

/// Часть возврата USDT 1 USDT `from` → `to` в состоянии `kind`: «идёт», «ждёт повтора», «вернулись», «не вернулись».
/// Паника — неизвестное состояние.
pub fn part(kind: &str, from: &str, to: &str) -> UsdtReturn {
    let (amount, from, to) = (Decimal::ONE, v(from), v(to));
    match kind {
        "pending" => UsdtReturn::Pending { amount, from, to },
        "retrying" => UsdtReturn::Retrying {
            amount,
            from,
            to,
            tries: 1,
            why: "тест".into(),
        },
        "done" => UsdtReturn::Done {
            amount,
            credited: amount,
            from,
            to,
            sent: None,
            fee_usd: None,
        },
        "not_returned" => UsdtReturn::NotReturned {
            amount,
            at: from,
            result_at_failure: None,
            why: "тест".into(),
        },
        _ => panic!("состояние части {kind}"),
    }
}

/// Сделка `id` закрыта с частью возврата `p` (ledger::save). Паника — сделки нет.
pub async fn closed_with(core: &Core, id: &str, p: UsdtReturn) {
    edit(core, id, |d| {
        d.step = Step::Closed;
        d.usdt_return = Some(p);
    })
    .await;
}

/// Сделка `id` закрыта, её выручка ждёт общего возврата группы `group` с площадки `from` (DealGroup.wait; сделка — не
/// первая группы). Паника — сделки нет.
pub async fn group_waits(core: &Core, id: &str, group: &str, from: &str) {
    let g = DealGroup {
        id: group.into(),
        wait: Some((v(from), Decimal::ONE)),
        ..Default::default()
    };
    edit(core, id, |d| {
        d.step = Step::Closed;
        d.group = Some(Box::new(g));
    })
    .await;
}

/// Сеть площадки `dex` в памяти ядра «держат сделки» (Inner.net_held). Ошибок нет.
pub fn net_held(core: &Core, dex: &str) -> bool {
    core.inner.net_held.lock().unwrap().contains(&v(dex))
}
