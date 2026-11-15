//! Файл: venues/bsc_dex/pools/load_izi.rs
//! Что делает: решение 286 — состояние пула iZiSwap / Biswap V3 на блоке: tokenX, tokenY и state() (первое слово —
//! sqrtPrice_96; ноль — пул не инициализирован) одним try_many; затем лесенка ответов САМОГО пула через виртуальный
//! квотер (quoteIzi, load_metric_ladder.rs read_with) на суммах dex.pmm_ladder_usd. Вид ядра — OraclePmm (как Metric):
//! своей математики точек и лимитных ордеров iZiSwap в боте нет — котировка и глубина по лесенке (точно в точках,
//! между ними — интерполяция). Порядок токенов пула — tokenX < tokenY (фабрика сортирует пару) = пара записи;
//! OraclePmmData.pool_token0 = tokenX (направление x2y для квотера и шага исполнителя).
//! Кто вызывает: pools/load.rs (ветки Proto::IziSwap, Proto::BiswapV3).
//! От чего зависит: ctx.rs, node/call.rs, tokens.rs, abi_izi.rs, load_metric_ladder.rs, quote/virtual_quoter.rs,
//! core/types.
//! Настройки: dex.pmm_ladder_usd [DEX] (через load_metric_ladder.rs; не задана — лесенки нет, пул без котировки).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{LadderPoint, OraclePmmData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::quote::virtual_quoter;
use super::super::tokens::Tokens;
use super::abi_izi::{IIziPool as P, IIziState as S};
use super::families::Family;
use super::load_metric_ladder::{self, Quoter};

/// Состояние пула на блоке. Принимает: окружение, токены, вид, пул, пару записи (по возрастанию), встречный токен
/// записи (Entry.usdt: USDT или промежуточная, решение 52), тег. Монета — сторона пары, не встречный токен (лесенка —
/// только у пары с USDT площадки, решение 297: load_metric_ladder.rs). Ошибки: узел, вызов откатился или ответ не
/// по ABI, токены пула — не пара записи (BadResponse).
#[allow(clippy::too_many_arguments)]
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    counter: Address,
    tag: &str,
) -> GatewayResult<PoolState> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("iZiSwap {pool}: {what}"));
    let calls: Vec<(Address, Bytes)> = vec![
        (pool, P::tokenXCall {}.abi_encode().into()),
        (pool, P::tokenYCall {}.abi_encode().into()),
        (pool, S::stateCall {}.abi_encode().into()),
    ];
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let word = |i: usize, what: &str| match &got[i] {
        Ok(b) if b.len() >= 32 => Ok(U256::from_be_slice(&b[..32])),
        Ok(_) => Err(bad(format!("{what}() не по ABI"))),
        Err(r) => Err(bad(format!("{what}() откатился: {r}"))),
    };
    let x = Address::from_word(word(0, "tokenX")?.into());
    let y = Address::from_word(word(1, "tokenY")?.into());
    let sqrt = word(2, "state")?;
    if (x, y) != pair {
        return Err(bad(format!("токены пула {x:#x} / {y:#x} — не пара записи")));
    }
    let coin = if pair.0 == counter { pair.1 } else { pair.0 };
    let quote = if pair.0 == coin { pair.1 } else { pair.0 };
    let q = Quoter {
        name: "iZiSwap",
        func: "quoteIzi",
        data: virtual_quoter::data_izi,
    };
    let (ladder_quote_in, ladder_base_in) = if sqrt.is_zero() {
        (Vec::new(), Vec::new())
    } else {
        load_metric_ladder::read_with(ctx, tokens, f.id, pool, coin, quote, x, tag, q).await?
    };
    Ok(state(pair, coin, sqrt, ladder_quote_in, ladder_base_in))
}

/// Сборка состояния (без сети; её же зовут тесты). Принимает: пару записи (tokenX, tokenY), монету, sqrtPrice_96 пула,
/// лесенки. Возвращает: OraclePmm (price — sqrtPrice_96 пула: ноль — пул пуст для quote/emptiness.rs). Ошибок нет.
pub fn state(
    pair: (Address, Address),
    coin: Address,
    sqrt_price_96: U256,
    ladder_quote_in: Vec<LadderPoint>,
    ladder_base_in: Vec<LadderPoint>,
) -> PoolState {
    PoolState::OraclePmm(Box::new(OraclePmmData {
        price: sqrt_price_96,
        spread: U256::ZERO,
        coeff: U256::ZERO,
        // Оракула нет: пригодность цены — инициализирован ли пул (sqrtPrice_96 ≠ 0); пуст пул — лесенки нет.
        feasible: !sqrt_price_96.is_zero(),
        paused: false,
        implementation: Address::ZERO,
        base_is_token0: pair.0 == coin,
        ladder_quote_in,
        ladder_base_in,
        pool_token0: Some(pair.0),
    }))
}
