//! Файл: tests/t489_common/deals.rs
//! Что делает: решение 489 — сделки тестов: идущая сделка bingx → площадка DEX (решение 196: держит площадку), её
//! изменение и сохранение, как ledger.rs save (учёт памяти, затем feed_hold::on_saved и net_hold::on_saved): закрытие,
//! закрытие с идущим возвратом USDT, конец возврата.
//! Кто вызывает: tests/core_net_chain_489_held.rs, tests/core_net_chain_489_bsc.rs.
//! От чего зависит: arb::core (deal, feed_hold, net_hold, usdt_return_state). Моков нет.

#![allow(dead_code)]

use rust_decimal::Decimal;
use serde_json::json;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::types::{Asset, ChainId, Direction, VenueId};
use arb::core::usdt_return_state::UsdtReturn;

/// Площадка по id. Ошибок нет.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Идущая сделка bingx → `dex` монеты сети `chain` (шаг «покупка» — открыта): по решению 196 держит площадку `dex`.
/// Ошибок нет.
pub fn hold(core: &Core, id: &str, dex: &str, chain: Option<ChainId>) {
    let dir = Direction {
        asset: Asset {
            chain,
            ticker: "USDT".into(),
            contract: None,
        },
        buy_on: v("bingx"),
        sell_on: v(dex),
    };
    let d = Deal::new(id.into(), dir, json!({}), Decimal::ZERO);
    core.inner.deals.lock().unwrap().insert(id.into(), d);
}

/// Меняет сделку `id` и сохраняет — как ledger.rs save: запись в учёт памяти, затем feed_hold::on_saved и
/// net_hold::on_saved. Возвращает: был ли толчок перечитывания справочника сетей (net_hold). Паника — сделки нет.
pub fn save(core: &Core, id: &str, f: impl FnOnce(&mut Deal)) -> bool {
    let mut deals = core.inner.deals.lock().unwrap();
    let d = deals.get_mut(id).unwrap();
    f(d);
    let rec = d.clone();
    drop(deals);
    arb::core::feed_hold::on_saved(core, &rec);
    arb::core::net_hold::on_saved(core)
}

/// Сделка `id` закрыта, возврат USDT не начат. Возвращает: был ли толчок (save). Паника — сделки нет.
pub fn close(core: &Core, id: &str) -> bool {
    save(core, id, |d| d.step = Step::Closed)
}

/// Сделка `id` закрыта, возврат USDT идёт (часть «идёт» `from` → `to`, как usdt_return_state.rs). Возвращает: был ли
/// толчок (save). Паника — сделки нет.
pub fn close_returning(core: &Core, id: &str, from: &str, to: &str) -> bool {
    let part = UsdtReturn::Pending {
        amount: Decimal::ONE,
        from: v(from),
        to: v(to),
    };
    save(core, id, |d| {
        d.step = Step::Closed;
        d.usdt_return = Some(part);
    })
}

/// Возврат USDT сделки `id` закончен (часть «вернулись»). Возвращает: был ли толчок (save). Паника — сделки нет.
pub fn return_done(core: &Core, id: &str, from: &str, to: &str) -> bool {
    let part = UsdtReturn::Done {
        amount: Decimal::ONE,
        credited: Decimal::ONE,
        from: v(from),
        to: v(to),
        sent: None,
        fee_usd: None,
    };
    save(core, id, |d| d.usdt_return = Some(part))
}
