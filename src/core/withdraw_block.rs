//! Файл: core/withdraw_block.rs
//! Что делает: решение 253, п. (г) — отказ вывода запоминается по паре «монета + биржа», а не только в сделке
//! (withdraw_refused.rs). Живой случай (Тигран 10.11): CYBER ×11 — после каждого отказа вывода с Binance следующая
//! сделка по монете шла тем же путём. Теперь: вывод сделки или перегона с биржи окончательно отказан (код из
//! withdraw_refused::refusal — WithdrawRejected / InvalidAddress / WithdrawLocked; повторы решения 166 уже сделаны в
//! transfers.rs) — биржа дописывается в coin.entry_blocked_venues монеты, причина — в coin.entry_blocked_note; журнал
//! core.withdraw_block и уведомление coin.withdraw_refused. Новые входы по монете с покупкой на этой бирже не
//! начинаются (check — в core/profit.rs до расчёта, как делистинг); прочие направления монеты — без изменений; идущие
//! сделки не трогаются. Снимает человек на экране «Монеты» (ui/src/components/WithdrawBlocked.svelte) — обе настройки
//! очищаются. Отказ вывода с DEX (откат перевода из кошелька) — не отказ площадки, не записывается (как mark_at).
//! Дополнение (Тигран 10.11): отказ из-за АККАУНТА, а не монеты — код InvalidAddress адаптера (адрес получателя не в
//! белом списке / адресной книге, по официальной таблице ошибок биржи: Binance −4007/−4012/−4033…−4036, Bybit
//! 131083/131093/131066/131098, OKX 58207 и другие коды InvalidAddress адаптера, Gate ADDRESS_NOT_USED) — монету НЕ закрывает: алерт
//! venue.account_problem «проблема в аккаунте <биржа>: <причина> — исправьте». У бирж без такого кода — как раньше.
//! Дополнение t82 (Тигран 10.11): так же — код AccountRestricted адаптера (вход, 2FA, пароль, верификация, блокировка
//! или лимит вывода аккаунта — по официальной таблице ошибок биржи, venues/<биржа>/errors.rs); причина человеческим
//! языком — notify/reason_codes_account.rs.
//! Дополнение t85 (Тигран 11.11): исчерпан лимит вывода аккаунта, который сбрасывается сам (WithdrawLimit) — монету НЕ
//! закрывает и алерта «проблема в аккаунте» не шлёт: площадка уже на паузе с алертом venue.withdraw_limit
//! (core/withdraw_limit_pause.rs из rules_apply.rs). Алерт venue.account_problem уходит и в чат «Аккаунты»
//! (notify.accounts_chat), если он задан (notify/routes.rs).
//! Кто вызывает: core/executor.rs, core/relay.rs, core/manual_relay.rs (после отказа вывода), core/profit.rs (check).
//! От чего зависит: withdraw_refused.rs (refusal), op_error.rs, trade_ops.rs (вид площадки), settings, deps.rs.
//! Настройки: coin.entry_blocked_venues, coin.entry_blocked_note (по монете — по адресу, решение 211; пишет ядро,
//! снимает человек).

use serde_json::Value;

use crate::core::types::{Asset, Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;
use super::op_error::OpError;

/// Список бирж, с которых входы по монете закрыты.
pub const VENUES: &str = "coin.entry_blocked_venues";
/// Причина закрытия (последний отказ).
pub const NOTE: &str = "coin.entry_blocked_note";
/// Кто меняет настройку (журнал изменений настроек).
const AUTHOR: &str = "ядро: вывод отклонён (решение 253)";

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
    // Дополнение t85: лимит вывода — монета ни при чём; пауза и алерт — withdraw_limit_pause.rs.
    if e.code == Some(ErrorCode::WithdrawLimit) {
        return;
    }
    // Дополнение (Тигран 10.11): отказ из-за аккаунта (адрес получателя: белый список, адресная книга — у адаптера код
    // InvalidAddress; состояние аккаунта — AccountRestricted, дополнение t82; оба — по официальной таблице ошибок
    // биржи) монету не закрывает — алерт «проблема в аккаунте».
    if matches!(
        e.code,
        Some(ErrorCode::InvalidAddress | ErrorCode::AccountRestricted)
    ) {
        return account(core, deal, asset, from, e).await;
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
        "Авто (решение 253): вывод {} с {} отклонён после повторов — {e} (сделка {deal})",
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

/// Что делает: отказ вывода из-за аккаунта (InvalidAddress — адрес; AccountRestricted — состояние аккаунта) — журнал
/// core.withdraw_block и алерт venue.account_problem шаблоном решения 251 (notify/human_msg.rs): «Проблема в аккаунте
/// <биржа>: <причина> — исправьте». Монета не закрывается. Принимает: ядро, id сделки, монету, биржу, ошибку. Ошибок нет.
async fn account(core: &Core, deal: &str, asset: &Asset, from: &VenueId, e: &OpError) {
    let deps = &core.inner.deps;
    let venue = crate::notify::human::venue(&from.0);
    let todo = if e.code == Some(ErrorCode::AccountRestricted) {
        format!(
            "исправьте в аккаунте {venue} то, что названо в причине (вход, 2FA, пароль, верификация, лимит)"
        )
    } else {
        format!(
            "исправьте в аккаунте {venue}: адрес кошелька DEX — в белом списке (адресной книге) вывода"
        )
    };
    let text = crate::notify::human_msg::Msg::event("venue.account_problem")
        .line(format!(
            "Проблема в аккаунте {venue}: вывод {} отклонён",
            asset.ticker
        ))
        .reason(&e.to_string())
        .bot("монету не закрывает (дело не в монете); сделка — как при отказе вывода")
        .todo(todo)
        .id(deal)
        .render();
    let log = format!(
        "{deal}: проблема в аккаунте {}: {e} — монета {} не закрыта",
        from.0, asset.ticker
    );
    deps.log(
        Level::Warn,
        "core.withdraw_block",
        Some(&from.0),
        &log,
        None,
    )
    .await;
    // Дополнение t85: и в чат роли «Аккаунты», если он задан (notify/routes.rs).
    deps.notify_tagged(
        "venue.account_problem",
        Some(&from.0),
        &text,
        crate::notify::routes::Tag::Plain,
    )
    .await;
}
