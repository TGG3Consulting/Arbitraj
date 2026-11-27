//! Завершает доказанный отказ и освобождает привязанный к исходной заявке nonce.
use super::super::parts::Parts;
use super::ledger::Marker;
use crate::{
    core::types::{TradeRequest, TradeResult},
    gateway::{errors::{ErrorCode, GatewayResult}, settlement_intent as intent},
};

/// Сохраняет нулевой результат для однозначного отказа первого POST или до сетевого вызова.
/// Принимает исходный маркер и причину для карточки и журнала. Конфликт исхода оставляет OutcomeUnknown.
pub(super) async fn declined(
    p: &Parts,
    mut r: TradeRequest,
    m: &Marker,
    reason: Option<&str>,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    let db = ctx.journal.db();
    let pending = m.pending();
    let unknown = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Ryze: требуется сверка записи отказа по исходному digest",
        )
        .pending(pending.clone())
    };
    let entry = intent::load(db, &m.key)
        .await
        .map_err(|_| unknown())?
        .ok_or_else(unknown)?;
    if entry.lookup != pending || entry.result.is_some() {
        return Err(unknown());
    }
    if let (Some(route), Some(reason)) = (&mut r.route, reason) {
        route.exec_note = Some(reason.to_owned());
    }
    let done = super::result::empty(r, m.digest);
    intent::confirm(
        db,
        &m.key,
        &entry,
        intent::Outcome::Order(Box::new(done.clone())),
    )
    .await
    .map_err(|_| unknown())?;
    if let Some(reason) = reason {
        let _ = ctx
            .journal
            .write(
                crate::storage::journal::Level::Warn,
                "ryze.relayer_declined",
                Some(ctx.place.id),
                reason,
                Some(serde_json::json!({"digest":m.digest,"deal_id":done.trade.request.deal_id})),
            )
            .await;
    }
    m.release(db).await.map_err(|_| unknown())?;
    intent::note_gross(Some(done.gross_quote));
    Ok(done.trade)
}
