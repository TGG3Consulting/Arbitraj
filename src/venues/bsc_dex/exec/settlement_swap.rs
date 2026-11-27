//! Файл: venues/bsc_dex/exec/settlement_swap.rs.
//! Что делает: до отправки денежного свопа связывает подпись с исходными токенами, сетью и кошельком.
//! Кто вызывает: адаптер DEX, прямой/умный путь и отправитель транзакций.
//! От чего зависит: SwapCtx, очередь nonce, исходная квитанция и ERC-20 Transfer.
//! Настройки: сеть, подписант и каналы площадки.
use super::super::{ctx::Ctx, parts::Parts, send, wallet};
use super::{chain_tx, fill, swap_ctx::SwapCtx};
use crate::core::types::{Asset, SendMethod, Side, TradeRequest, TradeResult, TxOutcome};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    pending::PendingRef,
};
use alloy_primitives::{Address, B256};
use rust_decimal::Decimal;
use std::future::Future;

tokio::task_local! { static TARGET: (Address, Address); }
tokio::task_local! { static PARSE: SwapCtx; }

/// Допускает подготовленный путь со штатной пробой, одним AMM-свопом и действующими разрешениями.
pub async fn trade(p: &Parts, req: TradeRequest, quote: &Asset) -> GatewayResult<TradeResult> {
    use crate::core::types::RouteMode;
    let ctx = &p.ctx;
    let token = req
        .asset
        .contract
        .ok_or_else(|| ctx.err(ErrorCode::NotConfigured, "Нет адреса целевого токена"))?;
    let cash = quote.contract.filter(|v| *v != token).ok_or_else(|| {
        ctx.err(
            ErrorCode::NotConfigured,
            "Нет отдельного контракта валюты свопа",
        )
    })?;
    if !crate::gateway::settlement_intent::active()
        || p.tokens.usdt(ctx)? != cash
        || !req.route.as_ref().is_some_and(|r| {
            r.executable
                && matches!(
                    r.mode,
                    RouteMode::Direct | RouteMode::Smart | RouteMode::Orderbook(_) | RouteMode::Relay(_)
                )
        })
    {
        return Err(ctx.err(ErrorCode::NoRoute,"Для денежного свопа требуется закреплённый расчётный актив и однотранзакционный маршрут AMM"));
    }
    if let Some(done)=super::super::pancakex::recovery::prior(p,&req).await? {return Ok(done);}
    if let Some(done)=super::super::ryze::recovery::prior(p,&req).await? {return Ok(done);}
    if let Some(RouteMode::Relay(id))=req.route.as_ref().filter(|r|super::super::pancakex::market::is_route(r)).map(|r|r.mode){return super::super::pancakex::execute::trade(p,req,id).await;}
    if let Some(RouteMode::Relay(id))=req.route.as_ref().map(|r|r.mode){return super::super::ryze::execute::trade(p,req,id).await;}
    if let Some(id) = req.route.as_ref().and_then(super::super::epsilon::routing::mixed_ticket) {
        let who = wallet::signer::address(ctx)?;
        let ticket = p.epsilon.take(who,id).ok_or_else(||ctx.err(ErrorCode::NoRoute,"Истёк проверенный расчёт Epsilon"))?;
        let result = super::super::epsilon::trade::trade(p,req,&ticket).await;
        return super::balance_fact::apply(ctx,&p.tokens,result).await;
    }
    if let Some(id) = req.route.as_ref().and_then(super::super::raindex::routing::mixed_ticket) {
        let who = wallet::signer::address(ctx)?;
        let ticket = p.raindex.take(who, id).ok_or_else(|| ctx.err(ErrorCode::NoRoute,"Истёк проверенный билет денежного ордера Raindex"))?;
        let result = super::super::raindex::settlement::trade(p, req, &ticket).await;
        return super::balance_fact::apply(ctx,&p.tokens,result).await;
    }
    if let Some(RouteMode::Orderbook(id)) = req.route.as_ref().map(|r| r.mode) {
        return super::super::hanji::execute::trade(p, req, id).await;
    }
    let sides = if req.side == Side::Buy {
        (cash, token)
    } else {
        (token, cash)
    };
    TARGET
        .scope(sides, async {
            let result = super::trade::trade(&p.quoting(), &p.env(), &p.nonces, req).await;
            super::balance_fact::apply(ctx, &p.tokens, result).await
        })
        .await
}

pub(super) async fn with_parse<T>(parse: &SwapCtx, f: impl Future<Output = T>) -> T {
    if TARGET.try_with(|_| ()).is_ok() {
        PARSE.scope(parse.clone(), f).await
    } else {
        f.await
    }
}

pub(super) fn marker(
    ctx: &Ctx,
    hash: B256,
    nonce: u64,
    ready: &[send::ready::Ready],
) -> GatewayResult<Option<PendingRef>> {
    let Ok(tokens) = TARGET.try_with(|v| *v) else {
        return Ok(None);
    };
    let mut parse = PARSE.try_with(Clone::clone).map_err(|_| {
        ctx.err(
            ErrorCode::NoRoute,
            "Своп требует отдельную транзакцию разрешения до денежного маршрута",
        )
    })?;
    if parse.tokens != Some(tokens)
        || ready.iter().any(|r| r.method != SendMethod::Single)
        || send::tip(ready).is_some()
    {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Токены или состав каналов изменились перед подписью",
        ));
    }
    let pending = send::track::Tracked::bare(TxOutcome::Pending, None);
    parse.chain = Some(chain_tx::chain_tx(
        hash,
        nonce,
        ready.iter().map(|r| r.ch.id.to_string()).collect(),
        SendMethod::Single,
        &pending,
        None,
    ));
    Ok(Some(PendingRef {
        nonce: Some(nonce),
        venue_ref: Some(hash.to_string()),
        parse: Some(
            serde_json::to_value(parse).map_err(|e| ctx.err(ErrorCode::Internal, e.to_string()))?,
        ),
        ..Default::default()
    }))
}

/// Резерв сохраняется до первого сетевого запроса даже при отмене future в send::dispatch.
pub(in crate::venues::bsc_dex) async fn reserve(
    ctx: &Ctx,
    slot: &wallet::nonce::Slot,
    hash: B256,
    nonce: u64,
) -> GatewayResult<()> {
    let w = ctx.wallet()?;
    let state = slot.hold().halt(
        "Ожидается подтверждение денежной операции".into(),
        nonce,
        hash,
    );
    let body =
        serde_json::to_string(&state).map_err(|e| ctx.err(ErrorCode::Internal, e.to_string()))?;
    crate::storage::wallet_queue::save(ctx.journal.db(), &w.queue_key, body)
        .await
        .map_err(|e| ctx.err(ErrorCode::OutcomeUnknown, e.to_string()))
}

/// Дебет или кредит токена кошелька из событий прямо этой транзакции, с возвратом излишка и без округления средней цены.
pub fn amount(
    receipt: &send::receipt::Receipt,
    wallet: Address,
    token: Address,
    side: Side,
    decimals: u8,
) -> Option<Decimal> {
    let debit = fill::moved(receipt, token, Some(wallet), None);
    let credit = fill::moved(receipt, token, None, Some(wallet));
    let units = match side {
        Side::Buy => debit.checked_sub(credit)?,
        Side::Sell => credit.checked_sub(debit)?,
    };
    super::super::num::to_decimal(units, decimals)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base_receipt_values_the_wallet_in_its_actual_cash_token() {
        let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../../tests/data/hashflow_direct_2026-11-26/swaps.json"
        ))
        .unwrap();
        let row = rows
            .iter()
            .find(|r| {
                r["transaction_hash"]
                    == "0xc285ebd7544a1f98920fd63edaf02f7d27c0a2a0d6943fcb3571a4aea687d72c"
            })
            .unwrap();
        let receipt: send::receipt::Receipt =
            serde_json::from_value(row["receipt"].clone()).unwrap();
        let wallet = row["from"].as_str().unwrap().parse().unwrap();
        let quote = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
            .parse()
            .unwrap();
        assert_eq!(
            amount(&receipt, wallet, quote, Side::Sell, 6),
            Some(Decimal::new(26602020, 6))
        );
        assert_eq!(amount(&receipt, wallet, quote, Side::Buy, 6), None);
        assert_eq!(
            amount(&receipt, Address::ZERO, quote, Side::Sell, 6),
            Some(Decimal::ZERO)
        );
    }
}
