//! Повтор денежного шага работает с исходным digest и его квитанцией, включая потерю ответа и перезапуск процесса.
use super::super::{exec, num, parts::Parts, quote::white_state, send};
use super::{
    chain, http,
    ledger::{self, Marker},
};
use crate::{
    core::types::{SendMethod, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
};
use alloy::primitives::B256;
use chrono::Utc;
use rust_decimal::Decimal;

fn same(r: &TradeRequest, s: &TradeRequest) -> bool {
    s.deal_id == r.deal_id
        && s.venue == r.venue
        && s.asset == r.asset
        && s.side == r.side
        && s.qty == r.qty
}
fn filled(
    p: &Parts,
    r: TradeRequest,
    m: &Marker,
    t: chain::Mined,
) -> GatewayResult<intent::SpotFill> {
    let bad = || {
        p.ctx.err(
            ErrorCode::OutcomeUnknown,
            "Ryze: не удалось выразить суммы выплаты",
        )
    };
    // Возврат того же входного актива от выбранного пула сохранён в фактах отдельно. Чужие платежи не улучшают результат.
    let net_input = t
        .fill
        .input_to_router
        .checked_sub(t.fill.input_from_path)
        .ok_or_else(bad)?;
    let (amount, cash) = if r.side == Side::Buy {
        (t.fill.output_from_router, net_input)
    } else {
        (net_input, t.fill.output_from_router)
    };
    let coins = num::to_decimal(amount, m.decimals.0)
        .filter(|v| *v > Decimal::ZERO)
        .ok_or_else(bad)?;
    let gross = num::to_decimal(cash, m.decimals.1).ok_or_else(bad)?;
    let tracked = send::track::Tracked {
        outcome: TxOutcome::Included,
        receipt: Some(t.receipt),
        cancel: None,
        unresolved: None,
    };
    let mut tx = exec::chain_tx::chain_tx(
        t.hash,
        t.tx_nonce,
        vec!["Ryze Relayer".into()],
        SendMethod::Single,
        &tracked,
        None,
    );
    // gas_used относится к целой транзакции релея. Чистый расход ETH кошелька на её включение равен нулю.
    tx.gas_bnb = Some(Decimal::ZERO);
    Ok(intent::SpotFill {
        trade: TradeResult {
            status: TradeStatus::Filled,
            filled_qty: coins,
            avg_price: gross.checked_div(coins).ok_or_else(bad)?,
            fees: vec![],
            venue_ref: t.hash.to_string(),
            chain: Some(tx),
            finished_at: Utc::now(),
            executor: Some("Ryze Relayer".into()),
            request: r,
        },
        gross_quote: gross,
    })
}
pub(super) fn empty(r: TradeRequest, id: B256) -> intent::SpotFill {
    intent::SpotFill {
        trade: TradeResult {
            request: r,
            status: TradeStatus::NotFilled,
            filled_qty: Decimal::ZERO,
            avg_price: Decimal::ZERO,
            fees: vec![],
            venue_ref: id.to_string(),
            chain: None,
            finished_at: Utc::now(),
            executor: Some("Ryze Relayer".into()),
        },
        gross_quote: Decimal::ZERO,
    }
}
pub(in crate::venues::bsc_dex) async fn reconcile(
    p: &Parts,
    r: TradeRequest,
    pend: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let ctx = &p.ctx;
    let db = ctx.journal.db();
    let unknown = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Ryze: требуется исход сохранённой подписи",
        )
        .pending(pend.clone())
    };
    let m = Marker::read(pend).ok_or_else(unknown)?;
    if ctx.chain_id() != Some(8453)
        || r.venue != ctx.venue()
        || ctx.account.as_deref() != Some(m.account.as_str())
    {
        return Err(unknown());
    }
    let entry = intent::load(db, &m.key)
        .await
        .map_err(|_| unknown())?
        .ok_or_else(unknown)?;
    if entry.lookup != *pend || !matches!(&entry.operation,intent::Request::Order(s) if same(s,&r))
    {
        return Err(unknown());
    }
    let saved = entry.result.as_ref().and_then(|o| match o {
        intent::Outcome::Order(f) => Some(f),
        _ => None,
    });
    let finished = |f: &intent::SpotFill| {
        intent::note_gross(Some(f.gross_quote));
        Some(f.trade.clone())
    };
    let result = if let Some(f) = saved {
        if f.trade.status == TradeStatus::NotFilled {
            m.release(db).await.map_err(|_| unknown())?;
            return Ok(finished(f));
        }
        let hash = f.trade.venue_ref.parse().map_err(|_| unknown())?;
        let t = chain::lookup(ctx, &m, hash)
            .await
            .map_err(|_| unknown())?
            .ok_or_else(unknown)?;
        if !m.save_fill(db,&t).await.map_err(|_|unknown())? {return Err(unknown());}
        let fresh = filled(p, r, &m, t).map_err(|_| unknown())?;
        if fresh.trade.filled_qty != f.trade.filled_qty || fresh.gross_quote != f.gross_quote {
            return Err(unknown());
        }
        m.release(db).await.map_err(|_| unknown())?;
        return Ok(finished(f));
    } else {
        let owned = m.holder(db).await.map_err(|_| unknown())? == Some(m.claim());
        let mut found = None;
        if owned {
            let url =
                http::endpoint(&m.relayer, &format!("intents/{}", m.digest)).ok_or_else(unknown)?;
            if let Some(id) = http::transaction(url, m.digest, ctx.limits()?).await {
                found = chain::lookup(ctx, &m, id).await.map_err(|_| unknown())?;
            }
            if found.is_none() {
                let tip = white_state::head(ctx, "latest")
                    .await
                    .map_err(|_| unknown())?;
                for id in chain::hashes(ctx, &m, tip).await.map_err(|_| unknown())? {
                    if let Some(t) = chain::lookup(ctx, &m, id).await.map_err(|_| unknown())? {
                        if found.replace(t).is_some() {
                            return Err(unknown());
                        }
                    }
                }
            }
        }
        if let Some(t) = found {
            if !m.save_fill(db,&t).await.map_err(|_|unknown())? {return Err(unknown());}
            filled(p, r, &m, t).map_err(|_| unknown())?
        } else if chain::unused_after_deadline(ctx, &m)
            .await
            .map_err(|_| unknown())?
        {
            empty(r, m.digest)
        } else {
            return Ok(None);
        }
    };
    intent::confirm(
        db,
        &m.key,
        &entry,
        intent::Outcome::Order(Box::new(result.clone())),
    )
    .await
    .map_err(|_| unknown())?;
    m.release(db).await.map_err(|_| unknown())?;
    Ok(finished(&result))
}

pub(in crate::venues::bsc_dex) async fn prior(
    p: &Parts,
    r: &TradeRequest,
) -> GatewayResult<Option<TradeResult>> {
    if p.ctx.chain_id() != Some(8453) || r.deal_id.is_empty() || r.asset.contract.is_none() {
        return Ok(None);
    }
    let key = ledger::identity(&p.ctx, r)?;
    let lookup = intent::load(p.ctx.journal.db(), &key)
        .await
        .map_err(|e| p.ctx.err(ErrorCode::OutcomeUnknown, e))?;
    let Some(e) = lookup.filter(|e| Marker::read(&e.lookup).is_some()) else {
        return Ok(None);
    };
    reconcile(p, r.clone(), &e.lookup)
        .await?
        .map(Some)
        .ok_or_else(|| {
            p.ctx
                .err(
                    ErrorCode::OutcomeUnknown,
                    "Ryze: новое исполнение ждёт исхода сохранённого поручения",
                )
                .pending(e.lookup)
        })
}
