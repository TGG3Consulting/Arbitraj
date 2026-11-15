//! Файл: venues/bsc_dex/pools/load_metric_ladder.rs
//! Что делает: решение 285 — «лесенка цен пула» Metric OMM на блоке (как WooFi, решение «122 F, 3, лесенка»): выход
//! САМОГО пула на лесенке объёмов dex.pmm_ladder_usd в обе стороны — проход 1: котируемая (USDT) → монета на суммах
//! лесенки; проход 2: монета → котируемая на полученных количествах. Ответ пула — через виртуальный квотер (решение 186,
//! contracts/VirtualQuoter.sol quoteMetric: пул исполняет свой swap, квотер откатывает всё в metricOmmSwapCallback с
//! дельтами), пачкой eth_call с подменой кода на одном блоке. Точка, где пул откатил (протухшая цена провайдера,
//! пауза, белый список расширения) или взял не весь вход (ликвидность кончилась), — край лесенки. Своих формул нет.
//! Кто вызывает: pools/load_metric.rs.
//! От чего зависит: ctx.rs, node/rpc.rs, tokens.rs, quote/virtual_quoter.rs, load_woo_ladder.rs (usd), core/types.
//! Настройки: dex.pmm_ladder_usd [DEX] (не задана — лесенки нет, пул без котировки).

use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};

use crate::core::types::LadderPoint;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::quote::virtual_quoter;
use super::super::tokens::Tokens;
use super::load_woo_ladder;

/// Обе стороны лесенки на блоке. Принимает: окружение, токены, id DEX, пул, монету, котируемую, USDT, token0 САМОГО
/// пула (порядок пула — для zeroForOne), тег. Возвращает: (котируемая → монета, монета → котируемая); пусто — лесенки
/// нет (не задана; котируемая не USDT). Ошибки: узел, неверная dex.pmm_ladder_usd, код квотера не сверен (Internal).
#[allow(clippy::too_many_arguments)]
pub async fn read(
    ctx: &Ctx,
    tokens: &Tokens,
    id: &str,
    pool: Address,
    coin: Address,
    quote: Address,
    usdt: Address,
    pool_token0: Address,
    tag: &str,
) -> GatewayResult<(Vec<LadderPoint>, Vec<LadderPoint>)> {
    let ladder = load_woo_ladder::usd(ctx, id)?;
    if ladder.is_empty() || quote != usdt {
        return Ok((Vec::new(), Vec::new()));
    }
    let dec = tokens.decimals(ctx, quote).await?;
    let unit = Decimal::from(10u64.pow(u32::from(dec)));
    let amounts: Vec<U256> = ladder
        .iter()
        .filter_map(|v| (*v * unit).trunc().to_u128().map(U256::from))
        .collect();
    let quote_in = pass(ctx, pool, quote == pool_token0, &amounts, tag).await?;
    let base_amounts: Vec<U256> = quote_in.iter().map(|p| p.amount_out).collect();
    let base_in = pass(ctx, pool, coin == pool_token0, &base_amounts, tag).await?;
    Ok((quote_in, base_in))
}

/// Один проход: quoteMetric(пул, направление, сумма) по суммам; точки — до первого отката, неполного входа или нуля.
/// Принимает: окружение, пул, zeroForOne пула, суммы, тег. Ошибки: узел (вся пачка), Internal (код квотера).
async fn pass(
    ctx: &Ctx,
    pool: Address,
    zero_for_one: bool,
    amounts: &[U256],
    tag: &str,
) -> GatewayResult<Vec<LadderPoint>> {
    if amounts.is_empty() {
        return Ok(Vec::new());
    }
    let code = virtual_quoter::runtime().map_err(|m| ctx.err(ErrorCode::Internal, m))?;
    let at = virtual_quoter::address();
    let params: Vec<(&str, Value)> = amounts
        .iter()
        .map(|a| {
            let data = virtual_quoter::data_metric(pool, zero_for_one, *a);
            (
                "eth_call",
                json!([{ "to": at, "input": data }, tag, { at.to_string(): { "code": code } }]),
            )
        })
        .collect();
    let got = rpc::batch(ctx, &params).await?;
    let mut out: Vec<LadderPoint> = Vec::new();
    for (a, r) in amounts.iter().zip(got) {
        // Откат пула (его причина — в тексте ошибки узла) — край лесенки.
        let Ok(raw) = r else { break };
        let Ok(b) = serde_json::from_str::<Bytes>(raw.get()) else {
            break;
        };
        let Some(Ok(o)) = virtual_quoter::decode(&b, *a) else {
            break;
        };
        if o.is_zero() || a.is_zero() || out.last().is_some_and(|p| p.amount_in >= *a) {
            break;
        }
        out.push(LadderPoint {
            amount_in: *a,
            amount_out: o,
        });
    }
    Ok(out)
}
