//! Файл: venues/bsc_dex/exec/bundle_trade.rs
//! Что делает: сделка по мульти-роутерному графовому маршруту бандлом свопов (решение 84, фаза 80.8c). Вызывается из
//! exec/trade.rs для RouteMode::Graph формы «бандл» (не прямая и не один-UR). Пересчитывает каждую долю по живому
//! состоянию пулов (exec/live.rs; перед этим проба крючка B/C и допуск — решение 32), проверяет среднюю цену против
//! худшей допустимой, ставит каждой доле её свежий amountOutMin из верификации (решение 83), собирает бандл
//! (exec/bundle.rs), занимает очередь кошелька местом Slot (резерв N nonce: первый = slot.nonce, дальше по порядку),
//! отправляет бандл (exec/bundle_send.rs — симуляция/газ/подпись/отправка/ожидание/settle), по итогу строит факт из
//! квитанций всех транзакций бандла (reuse exec/result.rs по объединённым логам). Повторов нет (стартовое правило
//! Тиграна): бандл истекает по maxBlockNumber, nonce свободны. Нативная монета в бандле v1 не поддерживается.
//! Классику/прямой/умный путь не трогает.
//! Кто вызывает: exec/trade.rs (графовый мульти-роутер).
//! От чего зависит: exec/{bundle,bundle_send,chain,live,native,result,verify_gate,txs,trade}.rs, quote/{via,direct}.rs,
//! wallet/nonce.rs, pools/registry.rs, core/types.
//! Настройки: dex.bundle_max_routers (исполнимость — quote/graph_route.rs); через вызываемые — send.*, dex.*, channel.*.

use rust_decimal::Decimal;

use alloy_primitives::{B256, U64, U256};

use crate::core::types::{ChainTx, DexRoute, SendMethod, Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::registry::Entry;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::send::receipt::Receipt;
use super::super::wallet::nonce::Nonces;
use super::chain::Step;
use super::trade::{check_price, sides, worst_positive};
use super::txs::{self, Env};
use super::verify_gate::Verified;
use super::{bundle, bundle_send, chain, live, native, result, verify_gate};

/// Сделка мульти-роутерным бандлом. Принимает: котировку, окружение, очередь кошелька, запрос, маршрут из запроса,
/// итог верификации (свежие amountOutMin по ногам). Ошибки: NoRoute (нативная/пустой/доля не годится), QuoteMismatch
/// (цена хуже допустимой), настройки, каналы, узел, SwapReverted/TxNotIncluded/OutcomeUnknown (с фактом бандла).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
    route: &DexRoute,
    verified: Verified,
) -> GatewayResult<TradeResult> {
    let ctx = &q.ctx;
    worst_positive(ctx, &req)?;
    if native::is_native(ctx, &req.asset) {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "нативная монета в мульти-роутерном бандле (решение 84) в v1 не исполняется",
        ));
    }
    let weights: Vec<Decimal> = route
        .legs
        .iter()
        .map(|l| {
            if req.side == Side::Buy {
                l.expected_out
            } else {
                l.share
            }
        })
        .collect();
    let total: Decimal = weights.iter().sum();
    if route.legs.is_empty() || total <= Decimal::ZERO {
        return Err(ctx.err(ErrorCode::NoRoute, "бандл: маршрут без долей"));
    }
    let (coin, usdt, decs) = sides(q, &req).await?;
    let out_token = if req.side == Side::Buy { coin } else { usdt };
    let out_dec = if req.side == Side::Buy {
        decs.0
    } else {
        decs.1
    };
    let taxes = direct::taxes(q, &req.asset)?;
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    // Доли по живому состоянию + свежий amountOutMin ноги (решение 83) в последний шаг; сверка средней цены с худшей.
    let mut legs: Vec<Vec<Step>> = Vec::with_capacity(route.legs.len());
    let (mut coins, mut paid, mut left) = (Decimal::ZERO, Decimal::ZERO, req.qty);
    for (i, (leg, w)) in route.legs.iter().zip(&weights).enumerate() {
        let qty = if i + 1 == route.legs.len() {
            left
        } else {
            (req.qty * w / total).min(left)
        };
        left -= qty;
        let path = live::path(&v, &leg.hops, qty, false).await?;
        coins += path.coins;
        paid += path.usdt;
        let mut steps = chain::steps(ctx, &req, &path, qty, decs)?;
        let min = verified.amount_out_min.get(i).copied().ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "верификация не дала минимум ноги бандла",
            )
        })?;
        verify_gate::apply_leg_min(ctx, &mut steps, min, out_dec)?;
        legs.push(steps);
    }
    check_price(ctx, &req, coins, paid)?;
    // Очередь кошелька: резерв N nonce (решение 45) — первый у slot.nonce, дальше по порядку в сборке бандла.
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let first_nonce = slot.nonce(ctx).await?;
    let (_at, now) = q.heads.at(ctx).await?;
    let b = bundle::build(ctx, &legs, out_token, wallet, first_nonce, now, false).await?;
    let entries: Vec<&Entry> = legs.iter().flatten().map(|s| &s.entry).collect();
    let sent = bundle_send::send(env, &slot, &b, &entries).await?;
    drop(slot);
    let tin = if req.side == Side::Buy { usdt } else { coin };
    bundle_result(ctx, req, &sent, wallet, (tin, out_token), decs, first_nonce)
}

/// Факт сделки бандла: один синтетический итог транзакции по ОБЪЕДИНЁННЫМ логам всех квитанций бандла и суммарному
/// газу — и общий разбор exec/result.rs (суммы Transfer монеты/USDT, средняя цена). Ошибки: как у result::result
/// (SwapReverted / TxNotIncluded / OutcomeUnknown с фактом; Included — факт по логам).
#[allow(clippy::too_many_arguments)]
fn bundle_result(
    ctx: &super::super::ctx::Ctx,
    req: TradeRequest,
    sent: &bundle_send::Sent,
    wallet: alloy_primitives::Address,
    (tin, tout): (alloy_primitives::Address, alloy_primitives::Address),
    decs: (u8, u8),
    first_nonce: u64,
) -> GatewayResult<TradeResult> {
    let outcome = sent.tracked.outcome;
    let gas_used: u64 = sent.receipts.iter().map(|r| r.gas_used.to::<u64>()).sum();
    let gas_bnb: Decimal = sent.receipts.iter().filter_map(|r| r.gas_bnb()).sum();
    let block = sent
        .receipts
        .iter()
        .map(|r| r.block_number.to::<u64>())
        .min();
    let logs = sent.receipts.iter().flat_map(|r| r.logs.clone()).collect();
    let chain = ChainTx {
        hash: *sent.hashes.last().unwrap_or(&B256::ZERO),
        nonce: first_nonce,
        channels: sent.channels.clone(),
        method: SendMethod::Bundle,
        outcome,
        block,
        gas_used: Some(gas_used),
        gas_bnb: Some(gas_bnb),
        tip_bnb: None,
        cancel_hash: None,
        cancel_gas_bnb: None,
    };
    // Квитанция для разбора — объединённые логи всех транзакций бандла (атомарен: либо все в блоке, либо ни одной).
    let receipt = (outcome == crate::core::types::TxOutcome::Included).then(|| Receipt {
        status: U64::from(1u8),
        block_number: U64::from(block.unwrap_or_default()),
        gas_used: U64::from(gas_used),
        effective_gas_price: U256::ZERO,
        logs,
    });
    let done = txs::Done {
        chain,
        receipt,
        cancel: None,
    };
    result::result(ctx, req, done, wallet, (tin, tout), decs, None)
}
