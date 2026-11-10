//! Файл: core/withdraw_block.rs
//! Что делает: решение 251, п. (г) — отказ вывода запоминается по паре «монета + биржа», а не только в сделке
//! (withdraw_refused.rs). Живой случай (Тигран 10.11): CYBER ×11 — после каждого отказа вывода с Binance следующая
//! сделка по монете шла тем же путём. Теперь: вывод сделки или перегона с биржи окончательно отказан (код из
//! withdraw_refused::refusal — WithdrawRejected / InvalidAddress / WithdrawLocked; повторы решения 166 уже сделаны в
//! transfers.rs) — биржа дописывается в coin.entry_blocked_venues монеты, причина — в coin.entry_blocked_note; журнал
//! core.withdraw_block и уведомление coin.withdraw_refused. Новые входы по монете с покупкой на этой бирже не
//! начинаются (check — в core/profit.rs до расчёта, как делистинг); прочие направления монеты — без изменений; идущие
//! сделки не трогаются. Снимает человек на экране «Монеты» (ui/src/components/WithdrawBlocked.svelte) — обе настройки
//! очищаются. Отказ вывода с DEX (откат перевода из кошелька) — не отказ площадки, не записывается (как mark_at).
//! Кто вызывает: core/executor.rs, core/relay.rs, core/manual_relay.rs (после отказа вывода), core/profit.rs (check).
//! От чего зависит: withdraw_refused.rs (refusal), op_error.rs, trade_ops.rs (вид площадки), settings, deps.rs.
//! Настройки: coin.entry_blocked_venues, coin.entry_blocked_note (по монете — по адресу, решение 211; пишет ядро,
//! снимает человек).

use serde_json::Value;

use crate::core::types::{Asset, Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;
use super::op_error::OpError;

/// Список бирж, с которых входы по монете закрыты.
pub const VENUES: &str = "coin.entry_blocked_venues";
/// Причина закрытия (последний отказ).
pub const NOTE: &str = "coin.entry_blocked_note";
/// Кто меняет настройку (журнал изменений настроек).
const AUTHOR: &str = "ядро: вывод отклонён (решение 251)";

/// Что делает: проверка до расчёта — входы по монете с биржи покупки направления не закрыты. Принимает: ядро,
/// направление. Ошибки: текст причины отказа («входы с binance закрыты: …»).
pub fn check(core: &Core, d: &Direction) -> Result<(), String> {
    let coin = crate::settings::scope::coin_id(&d.asset.ticker, d.asset.bsc_contract.as_ref());
    let s = &core.inner.deps.settings;
    let at = At {
        venue: None,
        coin: Some(&coin),
    };
    let list = s.list(VENUES, at).unwrap_or_default();
    if !list.contains(&d.buy_on.0) {
        return Ok(());
    }
    let note = s.text(NOTE, at).unwrap_or_default();
    Err(format!(
        "{}: входы с {} закрыты после отказа вывода ({VENUES}; снять — экран «Монеты»): {note}",
        d.asset.ticker, d.buy_on.0
    ))
}

/// Что делает: окончательный отказ вывода с биржи — закрыть входы монеты с неё (см. шапку). Принимает: ядро, id сделки,
/// монету (ядра), биржу вывода, ошибку. Ошибок нет: сбой записи — журнал и уведомление «внесите вручную».
pub async fn close(core: &Core, deal: &str, asset: &Asset, from: &VenueId, e: &OpError) {
    if !super::withdraw_refused::refusal(e.code) {
        return;
    }
    if super::trade_ops::kind(core, from).ok() == Some(VenueKind::Dex) {
        return;
    }
    let deps = &core.inner.deps;
    let scope = super::blacklist::coin_scope(asset);
    let coin = crate::settings::scope::coin_id(&asset.ticker, asset.bsc_contract.as_ref());
    let at = At {
        venue: None,
        coin: Some(&coin),
    };
    let mut list = deps.settings.list(VENUES, at).unwrap_or_default();
    if !list.contains(&from.0) {
        list.push(from.0.clone());
    }
    let note = format!(
        "Авто (решение 251): вывод {} с {} отклонён после повторов — {e} (сделка {deal})",
        asset.ticker, from.0
    );
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
            "{}: новые входы с {} закрыты — {note}. Снять — экран «Монеты»",
            asset.ticker, from.0
        ),
        Err(err) => format!(
            "{}: {note}; закрыть входы с {} не удалось ({err}) — закройте вручную",
            asset.ticker, from.0
        ),
    };
    deps.log(
        Level::Warn,
        "core.withdraw_block",
        Some(&from.0),
        &text,
        None,
    )
    .await;
    deps.notify("coin.withdraw_refused", Some(&from.0), &text)
        .await;
}
