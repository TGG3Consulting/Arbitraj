//! Файл: venues/bsc_dex/exec/trade_smart.rs
//! Что делает: сделка по умному маршруту из котировки (решение 38): пересчёт каждой доли по живому состоянию её пулов
//! (один пул или пара через промежуточную монету, решение 52 — exec/live.rs; перед этим — повторная проба крючка B/C,
//! решение 32), проверка средней цены против худшей допустимой, шаги и минимумы каждой доли (exec/chain.rs),
//! ожидаемые поступления выходного токена (wallet/inflows.rs), сверка долей с MixedQuoter PancakeSwap, разрешения
//! и один execute Universal Router PancakeSwap со всеми долями и итоговой проверкой баланса (exec/universal.rs),
//! ожидание итога своим узлом, факт по логам Transfer. Повторов нет (стартовое правило Тиграна).
//! Решение 121 B1 (как решение 99): если разрешения отправлялись (каждое ждёт блока), доли, минимумы и calldata
//! собираются ЗАНОВО на живом состоянии и снова сверяются с req.worst_price и MixedQuoter (у графа — повторная
//! верификация); свежей сборке нужно новое разрешение — своп не отправляется (QuoteMismatch).
//! Итог и ошибку подписывает исполнителем «свой сборщик: умный» вызывающий (exec/trade.rs, решение 121 B7).
//! Подготовка долей (пути, цена, MixedQuoter, минимумы) — exec/smart_legs.rs.
//! Кто вызывает: exec/trade.rs.
//! Графовый маршрут (решение 83): вызывающий (exec/trade.rs) передаёт свежие amountOutMin по ногам из предторговой
//! верификации (exec/verify_gate.rs) — они ставятся последнему шагу каждой доли; умный путь без графа передаёт None.
//! От чего зависит: check/sim_base.rs (блок пробы, решение 127), exec/live.rs, exec/chain.rs, exec/universal.rs, exec/precheck_sim.rs (кто меняет), exec/trade.rs (общие шаги), exec/verify_gate.rs, exec/result.rs,
//! quote/direct.rs, quote/via.rs, quote/mixed_quoter.rs, wallet/nonce.rs.
//! Настройки: через вызываемые — smart.*, exec.dex_slippage_pct, dex.*, verify.*, send.*, channel.*.
//!
//! Количество доли: продажа — qty × доля входа (RouteLeg.share); покупка — qty × ожидаемые монеты доли / все
//! ожидаемые монеты; последняя доля получает остаток, чтобы сумма долей была ровно qty.
//! Нативная монета BNB (решение 62) — exec/native.rs: одна WRAP_ETH на все доли продажи / одна UNWRAP_WETH покупки.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Side, TradeRequest, TradeResult};
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;

use super::super::check::sim_base::{self, Base};
use super::super::pools::registry::Entry;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::wallet::nonce::Nonces;
use super::native;
use super::plan::Plan;
use super::smart_legs::{Prep, prepare};
use super::trade::{
    no_new_approvals, rebuilt_notice, send_approvals, send_swap, sides, worst_positive,
};
use super::txs::Env;
use super::{precheck_sim, result, universal, verify_gate};

/// Сделка по умному маршруту. verify_mins — свежие amountOutMin по ногам графовой верификации (решение 83): Some —
/// ставятся последнему шагу каждой доли; None — умный путь без графа, минимумы считает exec/chain.rs. Ошибки: NoRoute
/// (доля не годится, пул пропал или не допущен), QuoteMismatch (цена хуже допустимой, расхождение с MixedQuoter,
/// число ног верификации не совпало, новое разрешение после approve), CoinNotChecked, настройки, каналы,
/// SwapReverted / TxNotIncluded / NonceConflict.
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
    route: &DexRoute,
    verify_mins: Option<Vec<Decimal>>,
) -> GatewayResult<TradeResult> {
    let ctx = &q.ctx;
    worst_positive(ctx, &req)?;
    let taxes = direct::taxes(q, &req.asset)?;
    let (coin, usdt, decs) = sides(q, &req).await?;
    let out_token = if req.side == Side::Buy { coin } else { usdt };
    let notice = |text: String| {
        env.stats.emit(MarketEvent::Notice {
            venue: ctx.venue(),
            code: "dex.swap_waits_inflow".into(),
            text,
        });
    };
    let guard = env.inflows.swap(ctx, out_token, &notice).await?;
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    let is_native = native::is_native(ctx, &req.asset);
    let mut prep = prepare(&v, &req, route, verify_mins.as_deref(), decs, is_native).await?;
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let (mut plan, mut at) = build(q, &prep, wallet).await?;
    if send_approvals(env, &slot, &plan.approvals).await? {
        // Решение 121 B1: разрешения ждали своих блоков — доли и минимумы устарели. Графу — свежая верификация, затем
        // пересчёт долей по живому состоянию, сверка с MixedQuoter и новая сборка execute.
        let mins = match verify_mins {
            Some(_) => Some(verify_gate::gate(q, env, &req, route).await?.amount_out_min),
            None => None,
        };
        prep = prepare(&v, &req, route, mins.as_deref(), decs, is_native).await?;
        (plan, at) = build(q, &prep, wallet).await?;
        no_new_approvals(ctx, &plan)?;
        rebuilt_notice(env, &req);
    }
    let router = is_native.then_some(plan.router);
    let entries: Vec<&Entry> = prep
        .paths
        .iter()
        .flat_map(|(p, _)| p.hops.iter().map(|h| &h.entry))
        .collect();
    precheck_sim::emitters(ctx, &plan, &entries, at).await?;
    let d = send_swap(env, &slot, plan, guard).await?;
    drop(slot);
    let tin = if req.side == Side::Buy { usdt } else { coin };
    result::result(
        ctx,
        req,
        d,
        wallet,
        (tin, out_token),
        decs,
        router.as_slice(),
    )
}

/// План execute на текущем блоке (exec/universal.rs): разрешения, которых не хватает сейчас. Принимает: котировку,
/// доли, кошелёк. Возвращает: план и базовый блок пробы (голова + 1 с, решение 127). Ошибки: NotConfigured, Internal, узел.
async fn build(
    q: &Quoting,
    prep: &Prep,
    wallet: alloy_primitives::Address,
) -> GatewayResult<(Plan, Base)> {
    let (at, now) = sim_base::head(&q.ctx, &q.heads).await?;
    let plan = universal::build(&q.ctx, &prep.steps, wallet, now)
        .await?
        .with_native(prep.wrap);
    Ok((plan, at))
}
