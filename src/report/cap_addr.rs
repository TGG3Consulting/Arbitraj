//! Файл: report/cap_addr.rs
//! Что делает: решение 256 (дополнение — владелец 11.11: «адреса чтобы были в сделках») — адреса и идентификаторы
//! сделки для листа «Сделки» отчёта «Сделки, лимиты и капитал». Только то, что есть в учёте сделки (core/deal.rs и
//! вложенные типы), ничего не достраивается:
//! - кошелёк DEX сделки — адрес по Deal.dex_account (id счёта, решение 132; адрес выводит адаптер из ключа —
//!   Venue::wallet_address, его читает report/cap_send.rs); нет id — пусто;
//! - пулы маршрута покупки / продажи на DEX — маршрут, отправленный в ордер (TradeResult.request.route: доли через
//!   « | », шаги через « → », у шага — вид пула и адрес: PoolRef — свой контракт, менеджер + id пула V4, общий
//!   контракт + монета); у цикла DEX ↔ DEX маршрут в ордер не пишется (route = None) — пулы из плана цикла
//!   (Deal.plan → CyclePlan.cycle.hops, core/cycle_plan.rs) с пометкой «по плану цикла»;
//! - покупка / продажи: биржа — id ордера (TradeResult.venue_ref), DEX — хеш транзакции (TradeResult.chain.hash);
//! - перевод монеты: вывод и зачисление (Deal.transfers парами: core/fills.rs transfer кладёт вывод, затем зачисление) —
//!   площадка, id на площадке, хеш в сети;
//! - хеши возврата USDT в учёте сделки не хранятся (UsdtReturn без хеша) — не показываются (примечание книги).
//!
//! Кто вызывает: report/cap_rows.rs.
//! От чего зависит: core/deal.rs, core/types (TradeResult, Transfer, DexRoute, PoolRef), notify/human.rs (venue).
//! Настройки: нет.

use std::collections::BTreeMap;

use crate::core::deal::Deal;
use crate::core::types::{DexRoute, PoolRef, TradeResult, Transfer};
use crate::notify::human::venue;

/// Адреса и идентификаторы сделки (пусто — в учёте нет).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Addr {
    pub wallet: String,
    pub buy_pools: String,
    pub sell_pools: String,
    pub buy_order: String,
    pub buy_tx: String,
    pub sell_orders: String,
    pub sell_txs: String,
    pub withdraws: String,
    pub deposits: String,
}

/// Адрес пула текстом (см. шапку). Ошибок нет.
pub fn pool(p: &PoolRef) -> String {
    match p {
        PoolRef::Contract(a) => format!("{a:#x}"),
        PoolRef::Singleton { manager, id } => format!("менеджер {manager:#x}, пул {id:#x}"),
        PoolRef::Member { contract, coin } => format!("{contract:#x} (монета {coin:#x})"),
    }
}

/// Маршрут ордера DEX текстом. Ошибок нет.
pub fn route(r: &DexRoute) -> String {
    let many = r.legs.len() > 1;
    let legs: Vec<String> = r
        .legs
        .iter()
        .map(|l| {
            let hops: Vec<String> = (l.hops.iter())
                .map(|h| format!("{} {}", h.family, pool(&h.pool)))
                .collect();
            let path = hops.join(" → ");
            if many {
                format!("доля {}: {path}", l.share.normalize())
            } else {
                path
            }
        })
        .collect();
    legs.join(" | ")
}

/// Пулы цикла DEX ↔ DEX из плана (CyclePlan.cycle.hops: family, pool). Нет в плане — пусто. Ошибок нет.
fn cycle_pools(plan: &serde_json::Value) -> String {
    let Some(hops) = plan.pointer("/cycle/hops").and_then(|h| h.as_array()) else {
        return String::new();
    };
    let v: Vec<String> = hops
        .iter()
        .filter_map(|h| {
            let p: PoolRef = serde_json::from_value(h.get("pool")?.clone()).ok()?;
            let fam = h.get("family").and_then(|f| f.as_str()).unwrap_or_default();
            Some(format!("{fam} {}", pool(&p)))
        })
        .collect();
    if v.is_empty() {
        return String::new();
    }
    format!("по плану цикла: {}", v.join(" → "))
}

/// Id ордера биржи (у DEX — пусто). Ошибок нет.
fn order(t: &TradeResult) -> String {
    if t.chain.is_some() {
        String::new()
    } else {
        t.venue_ref.clone()
    }
}

/// Хеш транзакции DEX (у биржи — пусто). Ошибок нет.
fn tx(t: &TradeResult) -> String {
    t.chain
        .as_ref()
        .map(|c| format!("{:#x}", c.hash))
        .unwrap_or_default()
}

/// Перевод текстом: площадка, id, хеш. Ошибок нет.
fn transfer(t: &Transfer) -> String {
    let h = t.tx_hash.clone().unwrap_or_else(|| "хеша нет".into());
    format!("{}: id {}, хеш {h}", venue(&t.venue.0), t.venue_ref)
}

/// Непустые через «; ». Ошибок нет.
fn join(v: impl Iterator<Item = String>) -> String {
    v.filter(|s| !s.is_empty()).collect::<Vec<_>>().join("; ")
}

/// Адреса сделки (см. шапку). Принимает: сделку, адреса кошельков по id счёта (Err — почему не прочитан). Ошибок нет.
pub fn of(d: &Deal, wallets: &BTreeMap<String, Result<String, String>>) -> Addr {
    let wallet = match &d.dex_account {
        Some(a) => match wallets.get(&a.0) {
            Some(Ok(x)) => x.clone(),
            Some(Err(e)) => format!("адрес не прочитан: {e}"),
            None => format!("кошелёк «{}» не найден в настройках", a.0),
        },
        None => String::new(),
    };
    let buy_route = d.buy.as_ref().and_then(|b| b.request.route.as_ref());
    let buy_pools = match buy_route {
        Some(r) => route(r),
        None if d.direction.buy_on == d.direction.sell_on => cycle_pools(&d.plan),
        None => String::new(),
    };
    let sells = &d.sells;
    Addr {
        wallet,
        buy_pools,
        sell_pools: join(
            sells
                .iter()
                .filter_map(|s| s.request.route.as_ref())
                .map(route),
        ),
        buy_order: d.buy.as_ref().map(order).unwrap_or_default(),
        buy_tx: d.buy.as_ref().map(tx).unwrap_or_default(),
        sell_orders: join(sells.iter().map(order)),
        sell_txs: join(sells.iter().map(tx)),
        withdraws: join(d.transfers.iter().step_by(2).map(transfer)),
        deposits: join(d.transfers.iter().skip(1).step_by(2).map(transfer)),
    }
}
