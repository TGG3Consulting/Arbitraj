//! Запись исходного хеша и nonce RFQ до первого сетевого вызова. Только открытые поля остаются в журнале.
use super::super::{ctx::Ctx, exec, parts::Parts, send, wallet};
use super::routing;
use crate::{
    core::types::{SendMethod, TradeRequest, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
};
use alloy_primitives::{B256, U256};
pub(super) struct Dispatch {
    pub(super) key: String,
    pub(super) expiry: u64,
    pub(super) gate: Gate,
    pub(super) parse: exec::swap_ctx::SwapCtx,
}

pub(super) enum Gate {
    Hashflow(String),
    Native(u64),
}
impl Dispatch {
    fn cap(&self, ctx: &Ctx) -> GatewayResult<U256> {
        let k = match self.gate {
            Gate::Hashflow(_) => "hashflow.gas_budget_native",
            Gate::Native(_) => "native_rfq.gas_budget_native",
        };
        super::super::num::to_units(ctx.dec(k)?, 18)
            .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Некорректный лимит газа RFQ"))
    }
    fn active(&self, ctx: &Ctx) -> bool {
        match &self.gate {
            Gate::Hashflow(source) => {
                super::enabled(ctx)
                    && ctx.text("hashflow.source").ok().as_ref() == Some(source)
                    && ctx
                        .int("hashflow.min_validity_ms")
                        .is_ok_and(|m| routing::expiry_ok(self.expiry, m))
            }
            Gate::Native(g) => {
                super::super::native_rfq::enabled(ctx)
                    && ctx.settings.generation() == *g
                    && super::super::native_rfq::routing::timely(ctx, self.expiry)
            }
        }
    }
}
tokio::task_local! {pub(super) static DISPATCH: Dispatch;}

/// Сверяет срок непосредственно перед сохранением хеша и передаёт в журнал восстановления только публичные поля.
pub(in crate::venues::bsc_dex) fn marker(
    ctx: &Ctx,
    hash: B256,
    nonce: u64,
    ready: &[send::ready::Ready],
    need: U256,
) -> GatewayResult<Option<PendingRef>> {
    DISPATCH
        .try_with(|d| {
            if need > d.cap(ctx)? || !d.active(ctx) || send::tip(ready).is_some() {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    "Допуск, срок RFQ или состав каналов изменился до отправки",
                ));
            }
            let how = if ready.iter().all(|r| r.method == SendMethod::Bundle) {
                SendMethod::Bundle
            } else {
                SendMethod::Single
            };
            let pending = send::track::Tracked::bare(TxOutcome::Pending, None);
            let chain = exec::chain_tx::chain_tx(
                hash,
                nonce,
                ready.iter().map(|r| r.ch.id.to_string()).collect(),
                how,
                &pending,
                None,
            );
            let mut parse = d.parse.clone();
            parse.chain = Some(chain);
            Ok(Some(PendingRef {
                nonce: Some(nonce),
                venue_ref: Some(hash.to_string()),
                parse: Some(serde_json::to_value(parse).map_err(|_| {
                    ctx.err(ErrorCode::Internal, "Не удалось сохранить контекст RFQ")
                })?),
                ..Default::default()
            }))
        })
        .unwrap_or(Ok(None))
}

/// До вызова любого канала сохраняет остановку очереди. При отмене future во время сетевого запроса nonce остаётся зарезервированным.
pub(in crate::venues::bsc_dex) async fn reserve(
    ctx: &Ctx,
    slot: &wallet::nonce::Slot,
    marker: &PendingRef,
) -> GatewayResult<()> {
    let Some(parse) =
        exec::swap_ctx::SwapCtx::of(marker).filter(|c| c.rfq.is_some() || c.native_rfq.is_some())
    else {
        return Ok(());
    };
    let chain = parse
        .chain
        .ok_or_else(|| ctx.err(ErrorCode::Internal, "Нет номера транзакции RFQ"))?;
    let w = ctx.wallet()?;
    let hold = slot.hold();
    let state = hold.halt(
        "Ожидается подтверждение отправки RFQ".into(),
        chain.nonce,
        chain.hash,
    );
    let body = serde_json::to_string(&state).map_err(|_| {
        ctx.err(
            ErrorCode::Internal,
            "Не сформирована запись очереди кошелька",
        )
    })?;
    let db = ctx.journal.db();
    crate::storage::wallet_queue::save(db, &w.queue_key, body)
        .await
        .map_err(|_| {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Не удалось сохранить резервирование nonce RFQ",
            )
            .pending(marker.clone())
        })?;
    let (valid, k) = DISPATCH.with(|d| (d.active(ctx), d.key.clone()));
    if valid {
        return Ok(());
    }
    let error = |_| {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Передачи в сеть не было, запись очереди требует сверки",
        )
        .pending(marker.clone())
    };
    let entry = intent::load(db, &k)
        .await
        .map_err(error)?
        .ok_or_else(|| error(String::new()))?;
    intent::archive_attempt(db, &k, &entry, "RFQ закрыта до первой сетевой отправки")
        .await
        .map_err(error)?;
    let free = hold.free();
    wallet::queue_store::save(ctx, &free).await;
    Err(ctx.err(
        ErrorCode::NoRoute,
        "Срок или допуск RFQ изменился во время резервирования до отправки",
    ))
}

pub(super) async fn submit<F>(
    p: &Parts,
    req: &TradeRequest,
    dispatch: Dispatch,
    send: F,
) -> GatewayResult<exec::txs::Done>
where
    F: std::future::Future<Output = GatewayResult<exec::txs::Done>>,
{
    let ctx = &p.ctx;
    let db = ctx.journal.db();
    let k = dispatch.key.clone();
    let parse = dispatch.parse.clone();
    let sent = intent::scope(
        db,
        &k,
        intent::Request::Order(Box::new(req.clone())),
        DISPATCH.scope(dispatch, send),
    )
    .await;
    match sent {
        Ok(d) => Ok(d),
        Err(e) if e.tx.is_some() => Err(parse.attach(e)),
        Err(e) => {
            if let Some(entry) = intent::load(db, &k).await.map_err(|_| {
                ctx.err(
                    ErrorCode::OutcomeUnknown,
                    "Проверка журнала отправки RFQ недоступна",
                )
            })? {
                return Err(ctx
                    .err(
                        ErrorCode::OutcomeUnknown,
                        "RFQ зарегистрирован перед отправкой; нужен исход по хешу",
                    )
                    .pending(entry.lookup));
            }
            Err(e)
        }
    }
}
pub(in crate::venues::bsc_dex) async fn native_send<F>(
    p: &Parts,
    req: &TradeRequest,
    expiry: u64,
    generation: u64,
    parse: &exec::swap_ctx::SwapCtx,
    send: F,
) -> GatewayResult<exec::txs::Done>
where
    F: std::future::Future<Output = GatewayResult<exec::txs::Done>>,
{
    let dispatch = Dispatch {
        key: super::execute::key(p, req)?,
        expiry,
        gate: Gate::Native(generation),
        parse: parse.clone(),
    };
    submit(p, req, dispatch, send).await
}
