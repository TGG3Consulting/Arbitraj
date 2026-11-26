//! Исполнение RFQT через существующую очередь nonce и разбор квитанций DEX.
use super::super::{ctx::Ctx, exec, parts::Parts, send, wallet};
use super::{routing, sim, wire};
use crate::core::types::{RouteMode, SendMethod, Side, TradeRequest, TradeResult, TxOutcome};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    events::MarketEvent,
    hashflow::Request,
    pending::PendingRef,
    settlement_intent as intent,
};
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Публичные поля, достаточные для подтверждения сделки по квитанции после перезапуска.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptProof {
    pool: Address,
    trader: Address,
    input: Address,
    output: Address,
    txid: B256,
    amount: U256,
    proceeds: U256,
}
impl ReceiptProof {
    pub(in crate::venues::bsc_dex) fn matches(&self, receipt: &send::receipt::Receipt) -> bool {
        let req = Request {
            trader: self.trader,
            input: self.input,
            output: self.output,
            amount: None,
            amount_tokens: None,
        };
        let mut logs = receipt
            .logs
            .iter()
            .filter(|l| l.address() == self.pool && l.topics() == [wire::Trade::SIGNATURE_HASH]);
        let event = logs.next().is_some_and(|l| {
            sim::event_matches(
                &l.inner.data.data,
                &req,
                self.txid,
                self.amount,
                self.proceeds,
            )
        }) && logs.next().is_none();
        // Штатный result использует именно эти два направления Transfer. Лишние движения кошелька расходятся с подписанной ценой.
        event
            && exec::fill::moved(receipt, self.input, Some(self.trader), None) == self.amount
            && exec::fill::moved(receipt, self.output, None, Some(self.trader)) == self.proceeds
            && exec::fill::moved(receipt, self.input, None, Some(self.trader)).is_zero()
            && exec::fill::moved(receipt, self.output, Some(self.trader), None).is_zero()
    }
}

struct Dispatch {
    expiry: u64,
    source: String,
    parse: exec::swap_ctx::SwapCtx,
}
tokio::task_local! {static DISPATCH: Dispatch;}

/// Сверяет срок непосредственно перед сохранением хеша и передаёт в журнал восстановления только публичные поля.
pub(in crate::venues::bsc_dex) fn marker(
    ctx: &Ctx,
    hash: B256,
    nonce: u64,
    ready: &[send::ready::Ready],
) -> GatewayResult<Option<PendingRef>> {
    DISPATCH
        .try_with(|d| {
            if !ctx.flag("hashflow.trading").unwrap_or(false)
                || ctx.text("hashflow.source")? != d.source
                || !routing::expiry_ok(d.expiry, ctx.int("hashflow.min_validity_ms")?)
                || send::tip(ready).is_some()
            {
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
                    ctx.err(
                        ErrorCode::Internal,
                        "Не удалось сохранить контекст Hashflow",
                    )
                })?),
                ..Default::default()
            }))
        })
        .unwrap_or(Ok(None))
}

/// Метка охватывает смену RFQ в одном денежном шаге. Повтор с другим размером после неопределённой отправки тоже требует сверки.
fn key(p: &Parts, r: &TradeRequest) -> GatewayResult<String> {
    let wallet = wallet::signer::address(&p.ctx)?;
    if r.deal_id.is_empty() {
        return Err(routing::no(
            p,
            "Для исполнения Hashflow нужен идентификатор сделки",
        ));
    }
    let coin = r
        .asset
        .contract
        .ok_or_else(|| routing::no(p, "У монеты отсутствует контракт"))?;
    let chain = super::super::node::chain_id::chain_id(&p.ctx)?;
    Ok(format!(
        "hashflow/{chain}/{wallet}/{coin}/{:?}/{}",
        r.side, r.deal_id
    ))
}

pub(super) async fn prior(p: &Parts, r: &TradeRequest) -> GatewayResult<Option<TradeResult>> {
    let db = p.ctx.journal.db();
    let Some(entry) = intent::load(db, &key(p, r)?)
        .await
        .map_err(|e| p.ctx.err(ErrorCode::Internal, e))?
    else {
        return Ok(None);
    };
    if let Some(intent::Outcome::Order(x)) = entry.result {
        if x.trade.request.qty == r.qty {
            return Ok(Some(x.trade));
        }
        return Err(routing::no(
            p,
            "Этот денежный шаг Hashflow уже завершён с другим объёмом",
        ));
    }
    Err(p
        .ctx
        .err(
            ErrorCode::OutcomeUnknown,
            "Сначала требуется сверить ранее подготовленную транзакцию Hashflow",
        )
        .pending(entry.lookup))
}

/// Фиксируется только подтверждённый по квитанции итог, после штатной проверки баланса ядром.
pub(in crate::venues::bsc_dex) async fn record(
    p: &Parts,
    t: TradeResult,
) -> GatewayResult<TradeResult> {
    if !matches!(
        t.request.route.as_ref().map(|r| r.mode),
        Some(RouteMode::Rfq(_))
    ) {
        return Ok(t);
    }
    let db = p.ctx.journal.db();
    let k = key(p, &t.request)?;
    let err = |_| {
        p.ctx
            .err(
                ErrorCode::OutcomeUnknown,
                "Не удалось подтвердить запись об уже отправленном Hashflow",
            )
            .pending(
                t.chain
                    .as_ref()
                    .map(|x| exec::swap_ctx::seed(x, None))
                    .unwrap_or_default(),
            )
    };
    let entry = intent::load(db, &k)
        .await
        .map_err(err)?
        .ok_or_else(|| err(String::new()))?;
    if let Some(intent::Outcome::Order(result)) = &entry.result {
        return Ok(result.trade.clone());
    }
    let parse = exec::swap_ctx::SwapCtx::of(&entry.lookup).ok_or_else(|| err(String::new()))?;
    let proof = parse.rfq.ok_or_else(|| err(String::new()))?;
    let (_, qd) = parse.decs.ok_or_else(|| err(String::new()))?;
    let gross = super::super::num::to_decimal(
        if t.request.side == Side::Buy {
            proof.amount
        } else {
            proof.proceeds
        },
        qd,
    )
    .ok_or_else(|| err(String::new()))?;
    let s = intent::SpotFill {
        trade: t.clone(),
        gross_quote: gross,
    };
    intent::confirm(db, &k, &entry, intent::Outcome::Order(Box::new(s)))
        .await
        .map_err(err)?;
    Ok(t)
}

pub(super) async fn trade(p: &Parts, req: TradeRequest, id: B256) -> GatewayResult<TradeResult> {
    if let Some(trade) = prior(p, &req).await? {
        return Ok(trade);
    }
    let ctx = &p.ctx;
    ctx.bound()?;
    let who = wallet::signer::address(ctx)?;
    let q = p.rfqs.take(who, id).ok_or_else(|| {
        routing::no(
            p,
            "Подписанная RFQ отсутствует в памяти кошелька; требуется новая котировка",
        )
    })?;
    if !ctx.flag("hashflow.trading").unwrap_or(false)
        || ctx.text("hashflow.source")? != q.source
        || !routing::timely(p, &q)
        || req.remeasure.as_ref().is_some_and(|r| !r.any_executor)
        || req.venue != ctx.venue()
        || req.qty <= Decimal::ZERO
        || req.worst_price <= Decimal::ZERO
    {
        return Err(routing::no(
            p,
            "RFQ больше не соответствует разрешённой сделке",
        ));
    }
    let chain = super::super::node::chain_id::chain_id(ctx)?;
    let coin = req
        .asset
        .contract
        .ok_or_else(|| routing::no(p, "У монеты отсутствует контракт"))?;
    let quote = p.tokens.usdt(ctx)?;
    let buy = req.side == Side::Buy;
    let (tokens, decs, coins, quoted) = if buy {
        ((quote, coin), (q.dout, q.di), q.output, q.input)
    } else {
        ((coin, quote), (q.di, q.dout), q.input, q.output)
    };
    if tokens != (q.request.input, q.request.output)
        || who != q.request.trader
        || super::super::num::to_decimal(coins, decs.0) != Some(req.qty)
    {
        return Err(routing::no(
            p,
            "Кошелёк, токены или количество расходятся с выбранной RFQ",
        ));
    }
    let actual = super::super::num::to_decimal(quoted, decs.1)
        .ok_or_else(|| routing::no(p, "Сумма RFQ вне диапазона"))?;
    let limit = req
        .worst_price
        .checked_mul(req.qty)
        .ok_or_else(|| routing::no(p, "Лимит сделки вне диапазона"))?;
    if (buy && actual > limit)
        || (!buy && actual < limit)
        || (!buy && req.remeasure.as_ref().is_some_and(|r| actual < r.floor))
    {
        return Err(ctx
            .err(
                ErrorCode::QuoteMismatch,
                "Подписанная цена Hashflow хуже границы сделки",
            )
            .price_moved());
    }
    let env = p.env();
    let notice = |text: String| {
        env.stats.emit(MarketEvent::Notice {
            venue: ctx.venue(),
            code: "dex.swap_waits_inflow".into(),
            text,
        })
    };
    let hold = env.inflows.swap(ctx, tokens.1, &notice).await?;
    let slot = p.nonces.acquire(ctx).await?;
    if slot.from != who || !routing::timely(p, &q) {
        return Err(routing::no(
            p,
            "Срок RFQ истёк в очереди кошелька или сменился отправитель",
        ));
    }
    let h = sim::header(ctx, chain)
        .await
        .map_err(|e| routing::no(p, &e))?;
    wire::verify(
        &q.firm,
        chain,
        super::router(chain).map_err(|e| routing::no(p, &e))?,
        &q.request,
        q.input,
        h.timestamp.to::<u64>(),
    )
    .map_err(|e| routing::no(p, &e))?;
    sim::run(ctx, h.hash, &q.request, &q.firm, q.input, q.output, q.value)
        .await
        .map_err(|e| routing::no(p, &e))?;
    let mut parse = exec::swap_ctx::SwapCtx::swap(who, tokens, decs, &[], None, None);
    let proof = ReceiptProof {
        pool: q.firm.quote_data.pool,
        trader: who,
        input: tokens.0,
        output: tokens.1,
        txid: id,
        amount: q.input,
        proceeds: q.output,
    };
    parse.rfq = Some(proof.clone());
    parse.who = Some("Hashflow RFQT".into());
    let dispatch = Dispatch {
        expiry: q.preview.expires,
        source: q.source.clone(),
        parse: parse.clone(),
    };
    let db = ctx.journal.db();
    let k = key(p, &req)?;
    let send = exec::txs::send(
        &env,
        &slot,
        q.firm.target_contract,
        q.firm.calldata.clone(),
        q.value,
        false,
        Some(hold),
    );
    let sent = intent::scope(
        db,
        &k,
        intent::Request::Order(Box::new(req.clone())),
        DISPATCH.scope(dispatch, send),
    )
    .await;
    let done = match sent {
        Ok(d) => d,
        Err(e) if e.tx.is_some() => return Err(parse.attach(e)),
        Err(e) => {
            if let Some(entry) = intent::load(db, &k).await.map_err(|_| {
                ctx.err(
                    ErrorCode::OutcomeUnknown,
                    "Проверка журнала отправки Hashflow недоступна",
                )
            })? {
                return Err(ctx
                    .err(
                        ErrorCode::OutcomeUnknown,
                        "Hashflow зарегистрирован перед отправкой; нужен исход по хешу",
                    )
                    .pending(entry.lookup));
            }
            return Err(e);
        }
    };
    if done.chain.outcome == TxOutcome::Included
        && !done.receipt.as_ref().is_some_and(|r| proof.matches(r))
    {
        return Err(parse.attach(
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Квитанция не подтверждает подписанный объём Hashflow",
            )
            .with_tx(done.chain),
        ));
    }
    exec::result::by(
        exec::result::result(ctx, req, done, who, tokens, decs, &[]).map_err(|e| parse.attach(e)),
        "Hashflow RFQT",
    )
}
