//! Отправка Hanji проходит через штатную очередь Base. До любого внешнего POST сохраняется один хеш.
use super::{
    book::{Config, header},
    recovery,
    wire::{self, Proof},
};
use crate::{
    core::types::{SendMethod, Side, TradeRequest, TradeResult},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        events::MarketEvent,
        pending::PendingRef,
        settlement_intent as intent,
    },
    venues::bsc_dex::{ctx::Ctx, exec, num, parts::Parts, send, wallet},
};
use alloy_primitives::{B256, U256, keccak256};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Marker {
    pub order: Proof,
    pub key: String,
    pub account: String,
    pub decs: (u8, u8),
    pub channels: Vec<String>,
    pub outer: bool,
}
tokio::task_local! {static SIGNING:(Marker,Config,U256);}
pub(super) fn key(p: &Parts, r: &TradeRequest) -> GatewayResult<String> {
    let ctx = &p.ctx;
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Для Hanji требуются сеть Base, ID сделки, счёт и контракты токенов",
        )
    };
    if ctx.chain_id() != Some(8453)
        || r.deal_id.is_empty()
        || r.asset.contract.is_none()
        || r.venue != ctx.venue()
    {
        return Err(bad());
    }
    let account = ctx.account.as_deref().ok_or_else(bad)?;
    Ok(intent::current_key().unwrap_or_else(|| {
        format!(
            "hanji/{}",
            keccak256(
                json!([
                    8453,
                    ctx.place.id,
                    account,
                    r.asset.contract,
                    r.side,
                    r.deal_id
                ])
                .to_string()
                .as_bytes()
            )
        )
    }))
}
pub(in crate::venues::bsc_dex) fn marker(
    ctx: &Ctx,
    hash: B256,
    nonce: u64,
    ready: &[send::ready::Ready],
    need: U256,
) -> GatewayResult<Option<PendingRef>> {
    SIGNING
        .try_with(|(m, c, cost)| {
            let allowed = ctx.settings.list("hanji.markets", ctx.at()).ok();
            if ctx.chain_id() != Some(8453)
                || !ctx.flag("hanji.trading").unwrap_or(false)
                || ctx.addr("hanji.executor").ok() != Some(m.order.executor)
                || ctx.dec("hanji.gas_budget_native").ok() != Some(c.gas)
                || !allowed
                    .is_some_and(|v| v.iter().any(|s| s.parse().ok() == Some(m.order.market)))
                || intent::current_key().as_deref() != Some(m.key.as_str())
                || Utc::now().timestamp().max(0) as u64 >= m.order.deadline
                || need > *cost
                || ready.is_empty()
                || ready.iter().any(|r| r.method != SendMethod::Single)
                || send::tip(ready).is_some()
            {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    "Hanji: изменились разрешения источника, стоимость, канал или срок подписи",
                ));
            }
            let mut m = m.clone();
            m.channels = ready.iter().map(|r| r.ch.id.to_string()).collect();
            Ok(Some(PendingRef {
                nonce: Some(nonce),
                venue_ref: Some(hash.to_string()),
                parse: Some(
                    serde_json::to_value(m)
                        .map_err(|e| ctx.err(ErrorCode::Internal, e.to_string()))?,
                ),
                ..Default::default()
            }))
        })
        .unwrap_or(Ok(None))
}
/// Смена выбранного источника или выключение Hanji не может повторить ранее отправленную ногу.
/// Проверяет индекс локального журнала только при начале реальной сделки на Base.
pub(in crate::venues::bsc_dex) async fn prior(
    p: &Parts,
    r: &TradeRequest,
) -> GatewayResult<Option<TradeResult>> {
    if p.ctx.chain_id() != Some(8453)
        || intent::active()
        || r.deal_id.is_empty()
        || r.asset.contract.is_none()
    {
        return Ok(None);
    }
    let k = key(p, r)?;
    let Some(entry) = intent::load(p.ctx.journal.db(), &k)
        .await
        .map_err(|e| p.ctx.err(ErrorCode::OutcomeUnknown, e))?
    else {
        return Ok(None);
    };
    let trade = recovery::reconcile(p, r.clone(), &entry.lookup)
        .await?
        .ok_or_else(|| {
            p.ctx
                .err(
                    ErrorCode::OutcomeUnknown,
                    "Перед выбором другого маршрута требуется подтвердить предыдущую подпись Hanji",
                )
                .pending(entry.lookup)
        })?;
    Ok(Some(trade))
}
/// Владелец успешного INSERT ещё не передал подпись. Если пока сохранялся nonce
/// допуск источника отозван, можно архивировать лишь свою нетронутую запись.
pub(in crate::venues::bsc_dex) async fn release_unsent(
    ctx: &Ctx,
    slot: &wallet::nonce::Slot,
    saved: &PendingRef,
    ready: &[send::ready::Ready],
    need: U256,
) -> GatewayResult<()> {
    let Ok(key) = SIGNING.try_with(|(m, _, _)| m.key.clone()) else {
        return Ok(());
    };
    let uncertain = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Hanji: подготовленная запись требует сверки",
        )
        .pending(saved.clone())
    };
    let hash = saved
        .venue_ref
        .as_deref()
        .and_then(|v| v.parse().ok())
        .ok_or_else(uncertain)?;
    let nonce = saved.nonce.ok_or_else(uncertain)?;
    let Err(error) = marker(ctx, hash, nonce, ready, need) else {
        return Ok(());
    };
    let db = ctx.journal.db();
    let entry = intent::load(db, &key)
        .await
        .map_err(|_| uncertain())?
        .ok_or_else(uncertain)?;
    if entry.lookup.parse != saved.parse
        || entry.lookup.venue_ref != saved.venue_ref
        || entry.lookup.nonce != Some(nonce)
        || entry.result.is_some()
    {
        return Err(uncertain());
    }
    intent::archive_attempt(
        db,
        &key,
        &entry,
        "Источник Hanji закрыт до первой передачи подписи",
    )
    .await
    .map_err(|_| uncertain())?;
    wallet::queue_store::save(ctx, &slot.hold().free()).await;
    Err(error)
}
pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    r: TradeRequest,
    id: B256,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    ctx.bound()?;
    let key = key(p, &r)?;
    let db = ctx.journal.db();
    let stale = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Подготовленная книга Hanji больше не соответствует сделке",
        )
    };
    let storage = |s: String| ctx.err(ErrorCode::OutcomeUnknown, s);
    let unknown = |m: &PendingRef| {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Транзакция Hanji ранее подготовлена. Требуется сверка исхода",
        )
        .pending(m.clone())
    };
    if let Some(e) = intent::load(db, &key).await.map_err(storage)? {
        return recovery::reconcile(p, r, &e.lookup)
            .await?
            .ok_or_else(|| unknown(&e.lookup));
    }
    let payer = wallet::signer::address(ctx)?;
    let t = p.hanji.tickets.take(payer, id).ok_or_else(stale)?;
    if t.book.expires <= Utc::now().timestamp_millis()
        || Config::read(p).as_ref() != Some(&t.book.config)
    {
        return Err(stale());
    }
    let mut o = t.order;
    let coin = r.asset.contract.ok_or_else(stale)?;
    let buy = r.side == Side::Buy;
    let (input, output) = if buy {
        (t.book.config.cash, coin)
    } else {
        (coin, t.book.config.cash)
    };
    let fixed = if buy { o.min_received } else { o.budget };
    if (input, output) != (o.input, o.output)
        || num::to_decimal(fixed, t.decs.0) != Some(r.qty)
        || r.qty <= rust_decimal::Decimal::ZERO
        || r.worst_price <= rust_decimal::Decimal::ZERO
        || r.remeasure.as_ref().is_some_and(|x| !x.any_executor)
    {
        return Err(stale());
    }
    let mut limit = r.worst_price.checked_mul(r.qty).ok_or_else(stale)?;
    if !buy && let Some(exit) = &r.remeasure {
        limit = limit.max(exit.floor);
    }
    let mut raw = num::to_units(limit, t.decs.1).ok_or_else(stale)?;
    if buy {
        o.budget = o.budget.min(raw);
        o.max_unspent = o.budget.checked_sub(U256::from(1)).ok_or_else(stale)?;
    } else {
        if num::to_decimal(raw, t.decs.1).ok_or_else(stale)? < limit {
            raw = raw.checked_add(U256::from(1)).ok_or_else(stale)?;
        }
        o.min_received = o.min_received.max(raw);
    }
    o.leg = keccak256(key.as_bytes());
    let env = p.env();
    let notice = |text: String| {
        env.stats.emit(MarketEvent::Notice {
            venue: ctx.venue(),
            code: "dex.swap_waits_inflow".into(),
            text,
        })
    };
    let hold = env.inflows.swap(ctx, output, &notice).await?;
    let slot = p.nonces.acquire(ctx).await?;
    if slot.from != payer {
        return Err(stale());
    }
    let h = header(ctx).await?;
    if h.timestamp.to::<u64>() >= o.deadline || Config::read(p).as_ref() != Some(&t.book.config) {
        return Err(stale());
    }
    wire::verify(ctx, &h, o.executor).await?;
    wire::simulate(ctx, &h, &o).await?;
    let outer = intent::active();
    let saved = Marker {
        order: o.clone(),
        key: key.clone(),
        account: ctx.account.as_deref().ok_or_else(stale)?.to_string(),
        decs: t.decs,
        channels: vec![],
        outer,
    };
    let bytes = o.data().ok_or_else(stale)?;
    let cap = num::to_units(t.gas, 18).ok_or_else(stale)?;
    let send = SIGNING.scope(
        (saved, t.book.config.clone(), cap),
        exec::txs::send(
            &env,
            &slot,
            o.executor,
            bytes,
            U256::ZERO,
            false,
            Some(hold),
        ),
    );
    let sent = if outer {
        send.await
    } else {
        intent::scope(db, &key, intent::Request::Order(Box::new(r.clone())), send).await
    };
    drop(slot);
    let entry = intent::load(db, &key).await.map_err(storage)?;
    let Some(entry) = entry else {
        return match sent {
            Err(e) => Err(e),
            Ok(_) => Err(storage("Hanji: отсутствует запись отправки".into())),
        };
    };
    recovery::reconcile(p, r, &entry.lookup)
        .await?
        .ok_or_else(|| unknown(&entry.lookup))
}
