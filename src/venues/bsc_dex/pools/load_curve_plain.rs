//! Файл: venues/bsc_dex/pools/load_curve_plain.rs
//! Что делает: решение 472 — чтение пула Curve StableSwap 2021 (вариант CurvePlain вида curve_stableswap_plain: 2pool
//! Arbitrum, Plain*Basic фабрики Curve Sidechain/L2) на блоке. Те же вызовы и разбор, что у Curve 2020
//! (load_curve.rs: coins(k), balances(k), затем fee()), но вместо `A()` — `A_precise()` (у этих пулов `A()` =
//! `_A() / A_PRECISION` — округлённый, а считает пул по `_A()`). Шаблон состояния: точность A = A_PRECISION = 100,
//! 255 раундов, несходимость — откат (`raise`), у пула из двух монет D_P одной строкой (StableData::pair_d).
//! Множители точности: у Plain*Basic — `rate_multipliers[i] = 10 ** (36 - decimals[i])` фабрики (геттера у пула
//! нет), у 2pool — константа RATE_MULTIPLIER = 10^30 при двух монетах по 6 знаков — то же правило 10^(36 − decimals),
//! что у Curve 2020 (load_curve::build). Пары token0/token1 — по адресам монет пула.
//! Кто вызывает: pools/load.rs, pools/load_site.rs (вариант CurvePlain); тест tests/arbitrum_curve_plain_live.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_many), tokens.rs, load_curve.rs (parse, build, Raw), abi_curve_plain.rs.
//! Настройки: нет (dex.multicall3 — через node/call.rs).
//!
//! Источники: curvefi/curve-factory f389123 Plain2Basic.vy — `A_PRECISION = 100` L78, `_A()` L230-L247, `A_precise()`
//! L264-L265, `_xp_mem` L270-L274, `get_D` L279-L310 (D_P L298, `raise` L310), `get_y` L452-L504, `get_dy` L509-L524,
//! `exchange` L530-L595; Plain3Basic / Plain4Basic того же коммита — D_P циклом (L299-L301). Фабрика fb61207
//! FactorySidechains.vy deploy_plain_pool — `rate_multipliers[i] = 10 ** (36 - decimals[i])`, decimals < 19. 2pool
//! Arbitrum (Vyper 0.2.12, Sourcify 42161 match): RATE_MULTIPLIER = 10^30, A_PRECISION = 100, get_D / get_y / exchange
//! — те же строки, что у Plain2Basic (сверка текста — DECISIONS 472).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_curve::{IStableSwapV1, MAX_COINS};
use super::abi_curve_plain::ICurvePlain;
use super::load_curve::{self, Raw};

/// Вызовы чтения: как load_curve::calls, но на месте `A()` — `A_precise()`. Возвращает: вызовы и индекс
/// A_precise(). Ошибок нет.
pub fn calls(pool: Address) -> (Vec<(Address, Bytes)>, usize) {
    let (mut calls, tail) = load_curve::calls(pool);
    calls[tail] = (pool, ICurvePlain::A_preciseCall {}.abi_encode().into());
    (calls, tail)
}

/// Шаблон Curve 2021 поверх состояния, собранного load_curve::build: A_PRECISION = 100, 255 раундов, откат при
/// несходимости, D_P пары у пула из двух монет. Принимает: состояние. Возвращает: его же с шаблоном. Ошибок нет.
pub fn plain(state: PoolState) -> PoolState {
    match state {
        PoolState::StableSwap(mut d) => {
            d.amp_precision = U256::from(100u8);
            d.max_rounds = 255;
            d.strict = true;
            d.pair_d = d.balances.len() == 2;
            PoolState::StableSwap(d)
        }
        other => other,
    }
}

/// Состояние пары на блоке. Принимает: окружение, токены (decimals), пул, пару (token0, token1), блок-тег.
/// Ошибки: узел, ответ не по ABI, монеты пары нет в пуле, decimals > 36, A_precise / fee откатились.
pub async fn load(
    ctx: &Ctx,
    tokens: &Tokens,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let raw = read(ctx, tokens, pool, tag).await?;
    state(ctx, &raw, pair)
}

/// Сетевое чтение пула без пары (решение 295): вызовы calls и decimals всех монет пула. Принимает: окружение, токены,
/// пул, блок-тег. Возвращает: Raw (тот же, что у load_curve). Ошибки: узел, ответов не столько, сколько вызовов,
/// ответ coins() не по ABI.
pub async fn read(ctx: &Ctx, tokens: &Tokens, pool: Address, tag: &str) -> GatewayResult<Raw> {
    let (calls, tail) = calls(pool);
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        let what = format!("Curve 2021 {pool}: ответов {} из {}", got.len(), calls.len());
        return Err(ctx.err(ErrorCode::BadResponse, what));
    }
    let mut decimals = Vec::new();
    for k in 0..MAX_COINS {
        let Some(Ok(c)) = got.get(2 * k) else { break };
        let coin = call::decode::<IStableSwapV1::coinsCall>(c)?;
        decimals.push(tokens.decimals(ctx, coin).await?);
    }
    Ok(Raw { pool, got, tail, decimals })
}

/// Состояние пары из прочитанного (без сети). Принимает: окружение (текст ошибки), Raw, пару. Ошибки: как у
/// load_curve::state_with (A_precise() разбирается как uint256 — тот же тип ответа, что A()).
pub fn state(ctx: &Ctx, raw: &Raw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    let pool = raw.pool;
    state_with(raw, pair, &|what: String| {
        ctx.err(ErrorCode::BadResponse, format!("Curve 2021 {pool}: {what}"))
    })
}

/// state с заданным конструктором ошибки (тесты на записанных ответах — без окружения). Ошибки: как у state.
pub fn state_with(
    raw: &Raw,
    pair: (Address, Address),
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    load_curve::state_with(raw, pair, bad).map(plain)
}
