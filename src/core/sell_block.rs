//! Файл: core/sell_block.rs
//! Что делает: решение 268, п. 6 (Тигран 14.11.2026; живой случай FAR 13.11: сделка в «ждём решения человека» не держит
//! замок монеты и не ставит паузу направления — бот ещё дважды вошёл в ту же монету после первого отказа продажи).
//! Продажа сделки на БИРЖЕ закончилась алертом «ждём решения человека» (sell.rs → sell_unsent::alert) — новые входы по
//! монете для направлений с ПРОДАЖЕЙ на этой бирже закрываются тем же механизмом, что отказ вывода (решение 253,
//! withdraw_block.rs): в coin.entry_blocked_venues монеты дописывается запись вида «sell:<биржа>» (записи решения 253 —
//! просто «<биржа>» — закрывают входы с ПОКУПКОЙ на бирже и не меняются), причина — в coin.entry_blocked_note; журнал
//! core.sell_block и уведомление coin.sell_blocked. Проверка до расчёта — withdraw_block::check (core/profit.rs).
//! Идущие сделки не трогаются. Снимает человек там же — экран «Монеты», блок «Входы закрыты» («Открыть sell:<биржа>»).
//! Запись уже есть — ничего не делает (без повторных уведомлений). DEX — не записывается.
//! Кто вызывает: core/sell.rs (перед алертом продажи), core/withdraw_block.rs (entry — в проверке).
//! От чего зависит: withdraw_block.rs (имена настроек), trade_ops.rs (вид площадки), blacklist.rs (область монеты),
//! settings, deps.rs, op_error.rs.
//! Настройки: sell.block_entry_on_halt (вся система; не задана или нет — ничего не делает); пишет
//! coin.entry_blocked_venues, coin.entry_blocked_note (по монете).

use serde_json::Value;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::withdraw_block::{NOTE, VENUES};

/// Настройка-включатель.
pub const ON: &str = "sell.block_entry_on_halt";
/// Приставка записи «входы с продажей на бирже закрыты» в coin.entry_blocked_venues.
pub const PREFIX: &str = "sell:";
/// Кто меняет настройку (журнал изменений настроек).
const AUTHOR: &str = "ядро: продажа остановлена (решение 268)";

/// Запись списка coin.entry_blocked_venues для биржи продажи. Принимает: биржу. Ошибок нет.
pub fn entry(venue: &VenueId) -> String {
    format!("{PREFIX}{}", venue.0)
}

/// Что делает: продажа сделки на бирже остановлена алертом — закрыть входы монеты с продажей на ней (см. шапку).
/// Принимает: ядро, сделку (монета, площадка, где лежат монеты, id), ошибку продажи (причина). Ошибок нет: сбой
/// записи — журнал и уведомление «закройте вручную».
pub async fn close(core: &Core, deal: &Deal, e: &OpError) {
    let deps = &core.inner.deps;
    if !deps.settings.flag(ON, At::default()).unwrap_or(false) {
        return;
    }
    if super::trade_ops::kind(core, &deal.at) != Ok(VenueKind::Cex) {
        return;
    }
    let asset = &deal.direction.asset;
    let coin = crate::settings::scope::coin_id(&asset.ticker, asset.bsc_contract.as_ref());
    let at = At {
        venue: None,
        coin: Some(&coin),
    };
    let mut list = deps.settings.list(VENUES, at).unwrap_or_default();
    let item = entry(&deal.at);
    if list.contains(&item) {
        return;
    }
    list.push(item);
    let note = format!(
        "Авто (решение 268): продажа {} на {} остановлена — {e} (сделка {})",
        asset.ticker, deal.at.0, deal.id
    );
    let scope = super::blacklist::coin_scope(asset);
    let s = &deps.settings;
    let vals: Vec<Value> = list.iter().cloned().map(Value::String).collect();
    let set = async {
        s.set(
            NOTE,
            scope.clone(),
            Some(Value::String(note.clone())),
            AUTHOR,
        )
        .await?;
        s.set(VENUES, scope, Some(Value::Array(vals)), AUTHOR).await
    };
    let text = match set.await {
        Ok(_) => format!(
            "{}: новые входы с продажей на {} закрыты — {note}. Снять — экран «Монеты»",
            asset.ticker, deal.at.0
        ),
        Err(err) => format!(
            "{}: {note}; закрыть входы с продажей на {} не удалось ({err}) — закройте вручную",
            asset.ticker, deal.at.0
        ),
    };
    deps.log(
        Level::Warn,
        "core.sell_block",
        Some(&deal.at.0),
        &text,
        None,
    )
    .await;
    deps.notify("coin.sell_blocked", Some(&deal.at.0), &text)
        .await;
}
