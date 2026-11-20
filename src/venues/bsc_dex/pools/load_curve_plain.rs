//! Файл: venues/bsc_dex/pools/load_curve_plain.rs
//! Что делает: решение 472 — чтение пула Curve StableSwap 2021 (вариант CurvePlain вида curve_stableswap_plain: 2pool
//! Arbitrum, Plain*Basic фабрики Curve Sidechain/L2) на блоке. Те же вызовы и разбор, что у Curve 2020
//! (load_curve.rs: coins(k), balances(k), затем fee()), но вместо `A()` — `A_precise()` (у этих пулов `A()` =
//! `_A() / A_PRECISION` — округлённый, а считает пул по `_A()`). Шаблон состояния: точность A = A_PRECISION = 100,
//! 255 раундов, несходимость — откат (`raise`), у пула из двух монет D_P одной строкой (StableData::pair_d).
//! Множители точности: у Plain*Basic — `rate_multipliers[i] = 10 ** (36 - decimals[i])` фабрики (геттера у пула
//! нет), у 2pool — константа RATE_MULTIPLIER = 10^30 при двух монетах по 6 знаков — то же правило 10^(36 − decimals),
//! что у Curve 2020 (load_curve::build). Пары token0/token1 — по адресам монет пула.
//! Кто вызывает: pools/load.rs, pools/load_site.rs (вариант CurvePlain), load_curve_oracle.rs (решение 480: read_calls,
//! state_with); тест tests/arbitrum_curve_plain_live.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_many), tokens.rs, load_curve.rs (parse, build, Raw), abi_curve_plain.rs,
//! stable_native.rs.
//! Настройки: dex.native_wrapped [DEX-площадка] (пул с ETH; не задана — такой пул не читается); dex.multicall3 — через
//! node/call.rs.
//!
//! Источники: curvefi/curve-factory f389123 Plain2Basic.vy — `A_PRECISION = 100` L78, `_A()` L230-L247, `A_precise()`
//! L264-L265, `_xp_mem` L270-L274, `get_D` L279-L310 (D_P L298, `raise` L310), `get_y` L452-L504, `get_dy` L509-L524,
//! `exchange` L530-L595; Plain3Basic / Plain4Basic того же коммита — D_P циклом (L299-L301). Фабрика fb61207
//! FactorySidechains.vy deploy_plain_pool — `rate_multipliers[i] = 10 ** (36 - decimals[i])`, decimals < 19. 2pool
//! Arbitrum (Vyper 0.2.12, Sourcify 42161 match): RATE_MULTIPLIER = 10^30, A_PRECISION = 100, get_D / get_y / exchange
//! — те же строки, что у Plain2Basic (сверка текста — DECISIONS 472).
//!
//! Решение 472 (доделка) — прочие реализации фабрики Curve Sidechain/L2 (curve-factory f389123, сверка текста
//! функций): Plain*Optimized — get_D / get_y те же, xp = балансы (rates = 10^18: initialize — `assert
//! _rate_multipliers[i] == PRECISION`, Plain2Optimized.vy L134) — тот же расчёт при rate_for_decimals(18) = 10^18;
//! Plain*Balances — get_D / get_y / _xp_mem те же, `balances(i)` = balanceOf − admin_balances (L231-L240), обмен
//! считает по ним же (exchange L570-L599) — геттер читается тем же вызовом, пул перечитывается каждым блоком;
//! Plain*ETH — coins[0] = 0xEeee… (initialize L132-L135, rate 10^18), обмен — msg.value / raw_call (L578-L606):
//! монета 0xEeee… в паре — dex.native_wrapped (stable_native.rs), decimals 18, индекс — StableData.native;
//! Plain2BasicEMA / Plain2ETHEMA (5272fd2) — те же get_D / get_y / dy, после — save_p: признак — ответ
//! `ma_exp_time()` (последний вызов пачки), StableData.ema (math/stable_ema.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_curve::{IStableSwapV1, MAX_COINS};
use super::abi_curve_plain::{ICurvePlain, ICurvePlainEma};
use super::load_curve::{self, Raw};
use super::stable_native::{self, NATIVE, NATIVE_DECIMALS};

/// Вызовы чтения: как load_curve::calls, но на месте `A()` — `A_precise()`, и последним — `ma_exp_time()` (ответ —
/// реализация с save_p; откат — без). Возвращает: вызовы и индекс A_precise(). Ошибок нет.
pub fn calls(pool: Address) -> (Vec<(Address, Bytes)>, usize) {
    let (mut calls, tail) = load_curve::calls(pool);
    calls[tail] = (pool, ICurvePlain::A_preciseCall {}.abi_encode().into());
    calls.push((pool, ICurvePlainEma::ma_exp_timeCall {}.abi_encode().into()));
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
    read_calls(ctx, tokens, pool, tag, &calls, tail).await
}

/// Решение 480: read с заданными вызовами (calls этого файла и сверху — свои вызовы варианта, load_curve_oracle.rs).
/// Принимает: окружение, токены, пул, блок-тег, вызовы, индекс A_precise(). Ошибки: как у read.
pub async fn read_calls(
    ctx: &Ctx,
    tokens: &Tokens,
    pool: Address,
    tag: &str,
    calls: &[(Address, Bytes)],
    tail: usize,
) -> GatewayResult<Raw> {
    let got = call::try_many(ctx, calls, tag).await?;
    if got.len() != calls.len() {
        let what = format!("Curve 2021 {pool}: ответов {} из {}", got.len(), calls.len());
        return Err(ctx.err(ErrorCode::BadResponse, what));
    }
    let mut decimals = Vec::new();
    for coin in coins(&got[..tail])? {
        // Метка нативного ETH (Plain*ETH): decimals() у неё нет — 18 по initialize (rate 10^18).
        decimals.push(if coin == NATIVE {
            NATIVE_DECIMALS
        } else {
            tokens.decimals(ctx, coin).await?
        });
    }
    Ok(Raw { pool, got, tail, decimals })
}

/// Монеты пула по ответам coins(k) / balances(k) через один (до первого отката coins). Ошибки: ответ не по ABI.
fn coins(head: &[Result<Bytes, String>]) -> GatewayResult<Vec<Address>> {
    let mut out = Vec::new();
    for k in 0..MAX_COINS {
        let Some(Ok(c)) = head.get(2 * k) else { break };
        out.push(call::decode::<IStableSwapV1::coinsCall>(c)?);
    }
    Ok(out)
}

/// Состояние пары из прочитанного (без сети). Принимает: окружение (текст ошибки), Raw, пару. Ошибки: как у
/// load_curve::state_with (A_precise() разбирается как uint256 — тот же тип ответа, что A()).
pub fn state(ctx: &Ctx, raw: &Raw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    let pool = raw.pool;
    state_with(raw, pair, stable_native::wrapped(ctx), &|what: String| {
        ctx.err(ErrorCode::BadResponse, format!("Curve 2021 {pool}: {what}"))
    })
}

/// state с заданной обёрткой нативной монеты и конструктором ошибки (тесты на записанных ответах — без окружения).
/// Пара — токены графа (ETH пула — обёртка): для разбора она переводится в монеты пула (обёртка → 0xEeee…).
/// Ошибки: как у state; у пула ETH, а обёртка не задана или тоже среди монет пула.
pub fn state_with(
    raw: &Raw,
    pair: (Address, Address),
    wrapped: Option<Address>,
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    let coins = coins(&raw.got[..raw.tail])?;
    stable_native::pair_coins(&coins, wrapped).map_err(bad)?;
    let native = stable_native::native_index(&coins);
    let sub = |t: Address| match (native, wrapped) {
        (Some(_), Some(w)) if t == w => NATIVE,
        _ => t,
    };
    let ema = matches!(raw.got.get(raw.tail + 2), Some(Ok(b)) if b.len() == 32);
    match plain(load_curve::state_with(raw, (sub(pair.0), sub(pair.1)), bad)?) {
        PoolState::StableSwap(mut d) => {
            d.native = native;
            d.ema = ema;
            Ok(PoolState::StableSwap(d))
        }
        other => Ok(other),
    }
}
