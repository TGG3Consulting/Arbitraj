//! Файл: venues/bsc_dex/pools/load_metric_ladder.rs
//! Что делает: решение 285 — «лесенка цен пула» Metric OMM на блоке (как WooFi, решение «122 F, 3, лесенка»): выход
//! САМОГО пула на лесенке объёмов dex.pmm_ladder_usd в обе стороны — проход 1: котируемая (USDT) → монета на суммах
//! лесенки; проход 2: монета → котируемая на полученных количествах. Ответ пула — через виртуальный квотер (решение 186,
//! contracts/VirtualQuoter.sol quoteMetric: пул исполняет свой swap, квотер откатывает всё в metricOmmSwapCallback с
//! дельтами), пачкой eth_call с подменой кода на одном блоке. Точка, где пул откатил (протухшая цена провайдера,
//! пауза, белый список расширения) или взял не весь вход (ликвидность кончилась), — край лесенки. Своих формул нет.
//! Решение 286: тот же проход для пулов iZiSwap / Biswap V3 (read_with с quoteIzi — load_izi.rs).
//! Решение 297: лесенка — только у пары, где котируемая = USDT площадки (tokens.rs usdt; load_woo_ladder.rs
//! usdt_quote), а не встречный токен записи (он бывает промежуточной монетой — WBNB). Решение 306: при
//! dex.pmm_ladder_counter = да — и пара не с USDT: суммы в котируемой по курсу к USDT (ladder_counter.rs amounts).
//! Кто вызывает: pools/load_metric.rs, pools/load_izi.rs.
//! От чего зависит: ctx.rs, node/rpc.rs, tokens.rs, registry.rs, quote/virtual_quoter.rs, ladder_counter.rs (amounts), core/types.
//! Настройки: dex.pmm_ladder_usd, dex.pmm_ladder_counter [DEX] (не задана — лесенки нет, пул без котировки).

use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

use crate::core::types::LadderPoint;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::quote::virtual_quoter;
use super::super::tokens::Tokens;
use super::ladder_counter;
use super::registry::Registry;

/// Обе стороны лесенки на блоке. Принимает: окружение, токены, реестр (курс встречного к USDT, решение 306), id DEX,
/// пул, монету, котируемую (встречный токен пары), token0 САМОГО пула (порядок пула — для zeroForOne), тег.
/// Возвращает: (котируемая → монета, монета → котируемая); пусто — лесенки нет (не задана; котируемая не USDT
/// площадки и dex.pmm_ladder_counter не «да» или нет курса — ladder_counter.rs, решения 297 и 306). Суммы прохода 1 —
/// в единицах котируемой. Ошибки: узел, неверная dex.pmm_ladder_usd, код квотера не сверен (Internal).
#[allow(clippy::too_many_arguments)]
pub async fn read(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    id: &str,
    pool: Address,
    coin: Address,
    quote: Address,
    pool_token0: Address,
    tag: &str,
) -> GatewayResult<(Vec<LadderPoint>, Vec<LadderPoint>)> {
    let q = Quoter {
        name: "Metric",
        func: "quoteMetric",
        data: virtual_quoter::data_metric,
    };
    read_with(ctx, tokens, reg, id, pool, coin, quote, pool_token0, tag, q).await
}

/// Вызов виртуального квотера для прохода лесенки: имя вида (в тексте ошибок) и вход вызова (пул, zeroForOne в
/// порядке токенов пула, сумма) — quoteMetric (решение 285) или quoteIzi (решение 286; x2y = вход tokenX).
#[derive(Clone, Copy)]
pub struct Quoter {
    pub name: &'static str,
    /// Имя функции квотера (в тексте ошибки «ответ не по ABI …»).
    pub func: &'static str,
    pub data: fn(Address, bool, U256) -> Bytes,
}

/// Как read, но с заданным вызовом квотера (решение 286). Принимает: те же аргументы и квотер. Возвращает и ошибки —
/// как у read.
#[allow(clippy::too_many_arguments)]
pub async fn read_with(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    id: &str,
    pool: Address,
    coin: Address,
    quote: Address,
    pool_token0: Address,
    tag: &str,
    q: Quoter,
) -> GatewayResult<(Vec<LadderPoint>, Vec<LadderPoint>)> {
    let amounts = ladder_counter::amounts(ctx, tokens, reg, id, quote).await?;
    if amounts.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let quote_in = pass(ctx, pool, quote == pool_token0, &amounts, tag, q).await?;
    let base_amounts: Vec<U256> = quote_in.iter().map(|p| p.amount_out).collect();
    let base_in = pass(ctx, pool, coin == pool_token0, &base_amounts, tag, q).await?;
    Ok((quote_in, base_in))
}

/// Один проход: quoteMetric(пул, направление, сумма) по суммам; точки — до первого отката пула, неполного входа или нуля.
/// Принимает: окружение, пул, zeroForOne пула, суммы, тег. Ошибки: узел (вся пачка или элемент — не откат пула),
/// BadResponse (ответ не по ABI), Internal (код квотера).
async fn pass(
    ctx: &Ctx,
    pool: Address,
    zero_for_one: bool,
    amounts: &[U256],
    tag: &str,
    q: Quoter,
) -> GatewayResult<Vec<LadderPoint>> {
    if amounts.is_empty() {
        return Ok(Vec::new());
    }
    let code = virtual_quoter::runtime().map_err(|m| ctx.err(ErrorCode::Internal, m))?;
    let at = virtual_quoter::address();
    let params: Vec<(&str, Value)> = amounts
        .iter()
        .map(|a| {
            let data = (q.data)(pool, zero_for_one, *a);
            (
                "eth_call",
                json!([{ "to": at, "input": data }, tag, { at.to_string(): { "code": code } }]),
            )
        })
        .collect();
    let got = rpc::batch(ctx, &params).await?;
    let mut out: Vec<LadderPoint> = Vec::new();
    for (a, r) in amounts.iter().zip(got) {
        // Ревью t117: откат пула — ответ узла с текстом «execution reverted» (geth core/vm/errors.go; venues/bsc_dex
        // errors.rs from_rpc → SwapReverted) — край лесенки. Любая другая ошибка элемента пачки (связь, состояние блока
        // удалено, лимит узла) — лесенка недостоверна: ошибка наверх (пул не прочитан → valid = false до следующего чтения).
        let raw = match r {
            Ok(raw) => raw,
            Err(e) if e.code == ErrorCode::SwapReverted => break,
            Err(e) => return Err(e),
        };
        let b = serde_json::from_str::<Bytes>(raw.get()).map_err(|e| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("квотер {}: ответ не bytes: {e}", q.name),
            )
        })?;
        let o = match virtual_quoter::decode(&b, *a) {
            Some(Ok(o)) => o,
            // Пул взял не весь вход — ликвидность кончилась: край лесенки.
            Some(Err(_)) => break,
            None => {
                return Err(ctx.err(
                    ErrorCode::BadResponse,
                    format!("квотер {}: ответ не по ABI {}", q.name, q.func),
                ));
            }
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
