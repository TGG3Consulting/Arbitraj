//! Файл: venues/bsc_dex/exec/bundle_trade.rs
//! Что делает: сделка по мульти-роутерному графовому маршруту бандлом свопов (решение 84, фаза 80.8c). Вызывается из
//! exec/trade.rs для RouteMode::Graph формы «бандл» (не прямая и не один-UR). Пересчитывает каждую долю по живому
//! состоянию пулов (exec/live.rs; перед этим проба крючка B/C и допуск — решение 32), проверяет среднюю цену против
//! худшей допустимой, ставит каждой доле её свежий amountOutMin из верификации (решение 83), собирает бандл
//! (exec/bundle.rs), занимает очередь кошелька местом Slot (резерв N nonce: первый = slot.nonce, дальше по порядку),
//! отправляет бандл (exec/bundle_send.rs — симуляция/газ/подпись/отправка/ожидание/settle), по итогу строит факт из
//! квитанций всех транзакций бандла (reuse exec/result.rs по объединённым логам). Повторов нет (стартовое правило
//! Тиграна): бандл истекает по maxBlockNumber, nonce свободны. Решение 233: исход не установлен — OutcomeUnknown с
//! «чем найти» (exec/bundle_ref.rs, контекст exec/swap_ctx.rs), позднее выяснение — resolve_bundle.rs. Нативная монета BNB (решение 122 R3) — обёртка/развёртка
//! WBNB командой каждого Universal Router бандла (exec/bundle_native.rs); факт — по WBNB Deposit / Withdrawal роутеров.
//! После сборки шлёт уведомление bundle.assembled (exec/bundle_notify.rs, 80.8c-iii): роутеров/ног/транзакций/первый nonce.
//! Решение 121 B1: разрешения здесь — транзакции того же атомарного бандла, что и свопы (exec/bundle.rs), в одном блоке;
//! между ними нет ожидания блока, поэтому пересборка после approve (как в прямом и умном пути) не нужна.
//! Итог и ошибку подписывает исполнителем «свой сборщик: бандл» вызывающий (exec/trade.rs, решение 121 B7). Классику/прямой/умный путь не трогает.
//! Решение 223: доли с общим пулом пересчитываются по порядку исполнения (quote/graph_seq.rs resequence).
//! Решение 228: минимум ноги — exec/leg_min.rs (E_k × max(1 − допуск, B / E), минимум верификации — нижней границей).
//! Кто вызывает: exec/trade.rs (графовый мульти-роутер).
//! От чего зависит: exec/{bundle,bundle_ref,swap_ctx,bundle_native,bundle_notify,bundle_send,chain,leg_min,live,native,result,verify_gate,txs,trade}.rs, quote/{via,direct}.rs,
//! wallet/nonce.rs, pools/registry.rs, core/types.
//! Настройки: dex.bundle_max_routers (исполнимость — quote/graph_route.rs); через вызываемые — send.*, dex.*, channel.*.

use rust_decimal::Decimal;

use alloy_primitives::{B256, U64, U256};

use crate::core::types::{ChainTx, DexRoute, SendMethod, Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::registry::Entry;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct, graph_seq};
use super::super::send::receipt::Receipt;
use super::super::wallet::nonce::Nonces;
use super::bundle_ref::{self, BundleRef};
use super::chain::Step;
use super::swap_ctx::SwapCtx;
use super::trade::{check_price, sides, worst_positive};
use super::txs::{self, Env};
use super::verify_gate::Verified;
use super::{
    bundle, bundle_native, bundle_notify, bundle_send, chain, leg_min, live, native, result,
};

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
    let is_native = native::is_native(ctx, &req.asset);
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
    let mut paths = Vec::with_capacity(route.legs.len());
    for (i, (leg, w)) in route.legs.iter().zip(&weights).enumerate() {
        let qty = if i + 1 == route.legs.len() {
            left
        } else {
            (req.qty * w / total).min(left)
        };
        left -= qty;
        paths.push((live::path(&v, &leg.hops, qty, false).await?, qty));
    }
    // Решение 223: доли с общим пулом — по порядку исполнения на состоянии пула после прошлых долей (бандл проводит их
    // по порядку внутри роутер-группы; доли с общим пулом — в одной группе). Без общих пулов — без изменений.
    graph_seq::resequence(ctx, req.side, &taxes, &mut paths)?;
    for (path, qty) in &paths {
        coins += path.coins;
        paid += path.usdt;
        legs.push(chain::steps(ctx, &req, path, *qty, decs)?);
    }
    check_price(ctx, &req, coins, paid)?;
    // Решение 228: минимум ноги = E_k × max(1 − допуск, B / E), свежий минимум верификации — нижней границей.
    let shares = paths.iter().map(|(p, q)| (p, *q));
    let ver = Some(verified.amount_out_min.as_slice());
    leg_min::set_leg_mins(ctx, &req, shares, &mut legs, ver, decs)?;
    // R3 (решение 122): BNB — первый шаг продажи платит роутер (WRAP_ETH), последний шаг покупки отдаёт роутеру.
    if is_native {
        bundle_native::adapt(ctx, req.side, &mut legs)?;
    }
    // Очередь кошелька: резерв N nonce (решение 45) — первый у slot.nonce, дальше по порядку в сборке бандла.
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let first_nonce = slot.nonce(ctx).await?;
    let (_at, now) = q.heads.at(ctx).await?;
    let b = bundle::build(ctx, &legs, out_token, wallet, first_nonce, now, false).await?;
    bundle_notify::assembled(
        &env.stats,
        ctx,
        b.swaps,
        legs.len(),
        b.txs.len(),
        first_nonce,
    );
    let entries: Vec<&Entry> = legs.iter().flatten().map(|s| &s.entry).collect();
    let sent = bundle_send::send(env, &slot, &b, &entries).await?;
    drop(slot);
    let tin = if req.side == Side::Buy { usdt } else { coin };
    let routers = if is_native {
        bundle_native::routers(&b)
    } else {
        Vec::new()
    };
    let nr = (first_nonce, routers.as_slice());
    // Решение 233: исход не установлен — «чем найти» бандла (хеши всех транзакций, nonce, кошелёк, контекст разбора по
    // образцу одиночной, exec/swap_ctx.rs) для позднего выяснения своим узлом (resolve_bundle.rs).
    let mut sc = SwapCtx::swap(wallet, (tin, out_token), decs, &routers, None, None);
    sc.bundle = Some(BundleRef::of(&sent, first_nonce));
    bundle_result(ctx, req, &sent, wallet, (tin, out_token), decs, nr).map_err(|e| sc.attach(e))
}

/// Факт сделки бандла: один синтетический итог транзакции по ОБЪЕДИНЁННЫМ логам всех квитанций бандла и суммарному
/// газу — и общий разбор exec/result.rs (суммы Transfer монеты/USDT, средняя цена). Ошибки: как у result::result
/// (SwapReverted / TxNotIncluded / OutcomeUnknown с фактом; Included — факт по логам); OutcomeUnknown — исход не
/// установлен или квитанций меньше, чем транзакций (exec/bundle_ref.rs, решение 233). Зовут также цикл
/// (exec/cycle_bundle_trade.rs) и позднее выяснение (exec/bundle_rebuild.rs).
#[allow(clippy::too_many_arguments)]
pub fn bundle_result(
    ctx: &super::super::ctx::Ctx,
    req: TradeRequest,
    sent: &bundle_send::Sent,
    wallet: alloy_primitives::Address,
    (tin, tout): (alloy_primitives::Address, alloy_primitives::Address),
    decs: (u8, u8),
    (first_nonce, native_routers): (u64, &[alloy_primitives::Address]),
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
    // Решение 233: исход не установлен (Pending) или квитанции прочитаны не все — «исход неизвестен», а не «не попала».
    if let Some(e) = bundle_ref::unknown(ctx, sent, &chain) {
        return Err(e);
    }
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
        tip_bnb: None,
        cancels: Vec::new(),
    };
    result::result(ctx, req, done, wallet, (tin, tout), decs, native_routers)
}
