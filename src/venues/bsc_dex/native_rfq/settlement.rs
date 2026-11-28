//! Устойчивые публичные условия исходной транзакции Native и сверка результата после перезапуска.
use super::{Profile, wire};
use crate::{
    core::types::{Side, TradeRequest, TradeResult, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        events::MarketEvent,
    },
    venues::bsc_dex::{
        ctx::Ctx,
        exec, hashflow,
        num,
        parts::Parts,
        quote::direct,
        send::receipt::Receipt,
        wallet,
    },
};
use alloy_primitives::{Address, B256, U256, keccak256};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub(super) order: wire::Proof,
    pub(super) module: Address,
    chain: u64,
    calldata: B256,
    minimum: U256,
}
impl Saved {
    pub(in crate::venues::bsc_dex) fn digest(&self) -> B256 { self.order.digest }
    pub(in crate::venues::bsc_dex) fn matches(&self, r: &Receipt) -> Option<U256> {
        self.order
            .received(&r.logs, Some(self.module))
            .filter(|n| *n >= self.minimum)
    }
    pub(in crate::venues::bsc_dex) fn wallet(&self, ctx: &Ctx) -> bool {
        ctx.chain_id() == Some(self.chain)
            && wallet::signer::address(ctx).ok() == Some(self.order.payer)
    }
    pub(in crate::venues::bsc_dex) async fn included(
        &self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        r: &Receipt,
    ) -> GatewayResult<()> {
        exec::signed_call::Erc20Call{chain:self.chain,payer:self.order.payer,module:self.module,calldata:self.calldata}.included(ctx,hash,nonce,r).await?;
        let fail=||ctx.err(ErrorCode::OutcomeUnknown,"Квитанция не подтвердила условия Native RFQ");
        if r.status.to::<u64>() == 1 && self.matches(r).is_none() {
            return Err(fail());
        }
        Ok(())
    }
    pub(in crate::venues::bsc_dex) fn sold(&self) -> U256 {
        self.order.sold
    }
}
pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    req: TradeRequest,
    t: &super::routing::Ticket,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    ctx.bound()?;
    let reject = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Предложение Native RFQ больше не соответствует выбранной сделке",
        )
    };
    let who = wallet::signer::address(ctx)?;
    let profile = Profile::read(ctx)?;
    if matches!(req.route.as_ref().map(|r|r.mode),Some(crate::core::types::RouteMode::NativeRfqSettlement(_)))
        && !profile.supports_settlement() { return Err(reject()); }
    let module = profile.module.ok_or_else(reject)?.address;
    let q = &t.checked.firm;
    let o = &q.proof;
    let buy = req.side == Side::Buy;
    let coin = req.asset.contract.ok_or_else(reject)?;
    let quote = p.tokens.usdt(ctx)?;
    let pair = if buy { (quote, coin) } else { (coin, quote) };
    if !super::enabled(ctx)
        || !t.valid(ctx)
        || req.qty != t.qty
        || req.side != t.side
        || req.venue != ctx.venue()
        || who != o.payer
        || pair != (o.input, o.output)
        || req.worst_price <= Decimal::ZERO
        || req.remeasure.as_ref().is_some_and(|r| !r.any_executor)
    {
        return Err(reject());
    }
    let wanted = direct::ceil_units(req.qty, t.decs.0).ok_or_else(reject)?;
    let cash = req.qty.checked_mul(req.worst_price).ok_or_else(reject)?;
    let minimum = if buy {
        if num::to_decimal(o.sold, t.decs.1).is_none_or(|n| n > cash) {
            return Err(reject());
        }
        wanted
    } else {
        if wanted != o.sold {
            return Err(reject());
        }
        let floor = req.remeasure.as_ref().map_or(cash, |r| cash.max(r.floor));
        direct::ceil_units(floor, t.decs.1).ok_or_else(reject)?
    }
    .max(o.minimum);
    let env = p.env();
    let hold = env
        .inflows
        .swap(ctx, o.output, &|text| {
            env.stats.emit(MarketEvent::Notice {
                venue: ctx.venue(),
                code: "dex.swap_waits_inflow".into(),
                text,
            })
        })
        .await?;
    let slot = p.nonces.acquire(ctx).await?;
    if slot.from != who || !t.valid(ctx) {
        return Err(reject());
    }
    super::simulate(ctx, &profile, q, minimum).await?;
    if !t.valid(ctx) {
        return Err(reject());
    }
    let data = q.call(minimum);
    let saved = Saved {
        order: o.clone(),
        module,
        chain: profile.chain,
        calldata: keccak256(&data),
        minimum,
    };
    let mut parse = exec::swap_ctx::SwapCtx::swap(who, pair, t.decs, &[], None, None);
    parse.native_rfq = Some(Box::new(saved.clone()));
    parse.who = Some(super::LABEL.into());
    let raw = exec::txs::send(&env, &slot, module, data, U256::ZERO, false, Some(hold));
    let d = hashflow::native_send(p, &req, q.expiry, t.generation, &parse, raw).await?;
    if matches!(d.chain.outcome, TxOutcome::Included | TxOutcome::Reverted) {
        let receipt = d.receipt.as_ref().ok_or_else(|| {
            parse.attach(
                ctx.err(
                    ErrorCode::OutcomeUnknown,
                    "Отсутствует квитанция Native RFQ",
                )
                .with_tx(d.chain.clone()),
            )
        })?;
        saved
            .included(ctx, d.chain.hash, d.chain.nonce, receipt)
            .await
            .map_err(|e| parse.attach(e.with_tx(d.chain.clone())))?;
    }
    exec::result::by(
        exec::result::with_native_fact(ctx, req, d, who, pair, t.decs, &[], Some(&parse)).map_err(|e| parse.attach(e)),
        super::LABEL,
    )
}
