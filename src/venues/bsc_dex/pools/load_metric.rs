//! Файл: venues/bsc_dex/pools/load_metric.rs
//! Что делает: решение 285 — состояние пула Metric OMM на блоке: getImmutables() (token0 САМОГО пула — порядок для
//! zeroForOne обмена) и getSellAndBuyPrices() (цены провайдера на этом блоке; откат — PriceProviderFailed: цена
//! протухла / рынок закрыт — обмен откатился бы тоже) одним try_many; при пригодной цене — лесенка ответов пула
//! (load_metric_ladder.rs). Вид ядра — OraclePmm (как WooFi): своей математики нет — котировка и глубина по лесенке.
//! Цена провайдера меняется каждым блоком — вид перечитывается на каждом новом блоке (per_block.rs).
//! Кто вызывает: pools/load.rs (ветка Proto::MetricOmm).
//! От чего зависит: ctx.rs, node/call.rs, tokens.rs, abi_metric.rs, load_metric_ladder.rs, core/types.
//! Настройки: dex.pmm_ladder_usd [DEX] (через load_metric_ladder.rs).
//!
//! Подписи — abi_metric.rs (SDK @nradko/metric-omm-sdk-v1@0.4.5 + селекторы в коде пула BSC, tests/data/metric_2026-11-15).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{OraclePmmData, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_metric::IMetricOmmPool as P;
use super::families::Family;
use super::load_metric_ladder;

/// Состояние пула на блоке. Принимает: окружение, токены, вид, пул, пару записи (по возрастанию), встречный токен
/// записи (Entry.usdt: USDT или промежуточная, решение 52), тег. Монета — сторона пары, не встречный токен (лесенка —
/// только у пары с USDT площадки, решение 297: load_metric_ladder.rs).
/// Ошибки: узел, getImmutables откатился или не по ABI, токены пула — не пара записи.
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
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Metric {pool}: {what}"));
    let calls: Vec<(Address, Bytes)> = vec![
        (pool, P::getImmutablesCall {}.abi_encode().into()),
        (pool, P::getSellAndBuyPricesCall {}.abi_encode().into()),
    ];
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let im = match &got[0] {
        Ok(b) => call::decode::<P::getImmutablesCall>(b)?,
        Err(r) => return Err(bad(format!("getImmutables откатился: {r}"))),
    };
    if !(pair == (im.token0, im.token1) || pair == (im.token1, im.token0)) {
        return Err(bad(format!(
            "токены пула {:#x} / {:#x} — не пара записи",
            im.token0, im.token1
        )));
    }
    // Откат getSellAndBuyPrices — провайдер цены не отвечает (PriceProviderFailed): цена непригодна, лесенки нет.
    let price = match &got[1] {
        Ok(b) => Some(call::decode::<P::getSellAndBuyPricesCall>(b)?),
        Err(_) => None,
    };
    let coin = if pair.0 == counter { pair.1 } else { pair.0 };
    let quote = if pair.0 == coin { pair.1 } else { pair.0 };
    let (ladder_quote_in, ladder_base_in) = if price.is_some() {
        load_metric_ladder::read(ctx, tokens, f.id, pool, coin, quote, im.token0, tag).await?
    } else {
        (Vec::new(), Vec::new())
    };
    Ok(state(
        pair,
        coin,
        im.token0,
        price.map(|p| p.sellPriceX64),
        ladder_quote_in,
        ladder_base_in,
    ))
}

/// Сборка состояния (без сети; её же зовут тесты). Принимает: пару записи, монету, token0 пула, цену продажи
/// провайдера X64 (None — откат), лесенки. Возвращает: OraclePmm. Ошибок нет.
pub fn state(
    pair: (Address, Address),
    coin: Address,
    pool_token0: Address,
    sell_price_x64: Option<u128>,
    ladder_quote_in: Vec<crate::core::types::LadderPoint>,
    ladder_base_in: Vec<crate::core::types::LadderPoint>,
) -> PoolState {
    PoolState::OraclePmm(Box::new(OraclePmmData {
        price: U256::from(sell_price_x64.unwrap_or(0)),
        spread: U256::ZERO,
        coeff: U256::ZERO,
        feasible: sell_price_x64.is_some(),
        // Паузы геттером не читаем (в ABI её нет): пул на паузе откатывает swap — лесенка пуста, котировки нет.
        paused: false,
        implementation: Address::ZERO,
        base_is_token0: pair.0 == coin,
        ladder_quote_in,
        ladder_base_in,
        pool_token0: Some(pool_token0),
    }))
}
