//! Файл: venues/bsc_dex/pools/load_curve_oracle.rs
//! Что делает: решение 481 (T472-10) — чтение пула Curve StableSwap с курсом оракула (вариант CurveOracle вида
//! curve_stableswap_oracle: ETH/wstETH Arbitrum 0x6eB2dc694eB516B16Dc9FBc678C60052BbdD7d80) на блоке. Пул — две монеты:
//! нативный ETH (метка 0xEeee…, coins(0)) и wstETH (coins(1)); `exchange(i, j)` считает курсами
//! `rates = [10^18, Oracle(ORACLE).latestAnswer()]` — курс оракула читается в САМОМ обмене (`_stored_rates`), кеша
//! нет. Поэтому чтение — то же, что у plain (load_curve_plain.rs: coins / balances / A_precise / fee /
//! ma_exp_time), плюс последним вызовом той же пачки — `latestAnswer()` оракула; курс монеты 1 = ответ
//! (отрицательный — пул откатил бы `convert(int256, uint256)`, ноль — деление на ноль в обмене: состояние не
//! собирается), курс монеты 0 = 10^(36 − 18) = 10^18 (то же правило load_curve::build). D_P — циклом (get_D L213-L239: `for _x in xp: D_P = D_P * D / (_x *
//! N_COINS)`), а не одной строкой, как у пары Plain2Basic. Адрес оракула — ответ `oracle()` пула при проверке
//! (verify_curve_plain.rs → remember), хранится в памяти по (площадка, пул) до перезапуска; нет в памяти — пул не
//! читается (не гадаем адрес). Пара — обёртка dex.native_wrapped вместо метки (stable_native.rs, как Plain*ETH).
//! Кто вызывает: pools/load.rs, pools/load_site.rs (вариант CurveOracle), pools/verify_curve_plain_judge.rs (remember).
//! От чего зависит: ctx.rs, node/call.rs (decode), tokens.rs, load_curve.rs (Raw), load_curve_plain.rs (calls,
//! read_calls, state_with), abi_curve_plain.rs (ICurveOraclePool, IRateOracle), stable_native.rs, gateway/shard_map.rs.
//! Настройки: dex.native_wrapped [DEX-площадка] (через stable_native.rs; не задана — пул с меткой ETH не читается).
//!
//! Источник: docs/sources/curve_arbitrum/0x6eB2dc694eB516B16Dc9FBc678C60052BbdD7d80.vy (Sourcify 42161, Vyper 0.3.1;
//! номера строк — исходника Sourcify): `__init__` L141-L156 (`assert _coins[0] == 0xEeee…` L144, `ORACLE = _oracle`
//! L156), `_A()` L161-L178 (тот же текст, что Plain2Basic), `A_precise()` L189-L190, `_stored_rates()` L193-L199,
//! `_xp` L204-L208, get_D L213-L239, get_y L370-L411 (тот же расчёт, что Plain2Basic), get_dy L416-L425 (`dy = xp[j] −
//! y` — БЕЗ «− 1»), exchange L444-L484 (`dy = xp[j] − y − 1` L461, комиссия с dy до перевода L462, выход `(dy − dy_fee)
//! · PRECISION / rates[j]` L468; вход ETH — `msg.value == dx` L473, выход ETH — `raw_call(msg.sender, value=dy)` L478),
//! `oracle()` L851-L853. Бот считает выход `exchange` (math/stableswap_exchange.rs) — квотер пула get_dy больше нашего
//! выхода на «− 1» (не больше чем на 1 единицу монеты j после деления на курс).

use std::sync::LazyLock;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::shard_map::ShardMap;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_curve_plain::{ICurveOraclePool, IRateOracle};
use super::load_curve::Raw;
use super::load_curve_plain;
use super::stable_native;

/// Оракул курса пула по (площадка DEX, пул) — из проверки (verify_curve_plain_judge.rs).
static ORACLES: LazyLock<ShardMap<(&'static str, Address), Address>> =
    LazyLock::new(ShardMap::default);

/// Запомнить оракул пула (ответ `oracle()` пула при проверке). Ошибок нет.
pub fn remember(venue: &'static str, pool: Address, oracle: Address) {
    ORACLES.insert((venue, pool), oracle);
}

/// Оракул пула из памяти. None — пул ещё не проверялся в этом запуске. Ошибок нет.
pub fn oracle(venue: &'static str, pool: Address) -> Option<Address> {
    ORACLES.get(&(venue, pool))
}

/// Вызов `oracle()` пула (проверка). Ошибок нет.
pub fn oracle_call(pool: Address) -> (Address, Bytes) {
    (pool, ICurveOraclePool::oracleCall {}.abi_encode().into())
}

/// Ответ `oracle()` пула. Возвращает: адрес; Err(None) — откат или ноль (у пула нет оракула — не этот исходник);
/// Err(Some) — сбой узла / ответ не по ABI. Ошибок иных нет.
pub fn oracle_of(answer: Option<call::Piece>) -> Result<Address, Option<String>> {
    let bytes: Bytes = match answer {
        Some(Ok(Ok(b))) => b,
        Some(Ok(Err(_))) => return Err(None),
        Some(Err(node)) => return Err(Some(node)),
        None => return Err(Some("нет ответа oracle() пула".into())),
    };
    let a = ICurveOraclePool::oracleCall::abi_decode_returns(&bytes)
        .map_err(|e| Some(format!("ответ oracle() не по ABI: {e}")))?;
    if a == Address::ZERO {
        return Err(None);
    }
    Ok(a)
}

/// Вызовы чтения: вызовы plain (load_curve_plain::calls) и последним — latestAnswer() оракула.
/// Возвращает: вызовы и индекс A_precise(). Ошибок нет.
pub fn calls(pool: Address, oracle: Address) -> (Vec<(Address, Bytes)>, usize) {
    let (mut calls, tail) = load_curve_plain::calls(pool);
    calls.push((oracle, IRateOracle::latestAnswerCall {}.abi_encode().into()));
    (calls, tail)
}

/// Сетевое чтение пула без пары: вызовы calls и decimals монет (метка ETH — 18). Принимает: окружение, токены, пул,
/// блок-тег. Ошибки: оракул не подтверждён проверкой, узел, ответов не столько, сколько вызовов, ответ не по ABI.
pub async fn read(ctx: &Ctx, tokens: &Tokens, pool: Address, tag: &str) -> GatewayResult<Raw> {
    let Some(oracle) = oracle(ctx.place.id, pool) else {
        let what =
            format!("пул Curve с оракулом {pool}: оракул не подтверждён проверкой в этом запуске");
        return Err(ctx.err(ErrorCode::BadResponse, what));
    };
    let (calls, tail) = calls(pool, oracle);
    load_curve_plain::read_calls(ctx, tokens, pool, tag, &calls, tail).await
}

/// Состояние пары на блоке. Принимает: окружение, токены, пул, пару, блок-тег. Ошибки: как у read и state.
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

/// Состояние пары из прочитанного (без сети). Ошибки: как у state_with.
pub fn state(ctx: &Ctx, raw: &Raw, pair: (Address, Address)) -> GatewayResult<PoolState> {
    let pool = raw.pool;
    state_with(raw, pair, stable_native::wrapped(ctx), &|what: String| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("пул Curve с оракулом {pool}: {what}"),
        )
    })
}

/// Состояние plain (load_curve_plain::state_with: метка ETH — обёртка) с курсом монеты 1 = latestAnswer() оракула и
/// D_P циклом. Принимает: Raw, пару (токены графа), обёртку нативной монеты, конструктор ошибки. Ошибки: как у
/// load_curve_plain::state_with; монет не две; монета 0 — не метка ETH (исходник: `assert _coins[0] == 0xEeee…`);
/// latestAnswer откатился, не по ABI, ≤ 0.
pub fn state_with(
    raw: &Raw,
    pair: (Address, Address),
    wrapped: Option<Address>,
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    let answer = match raw.got.last() {
        Some(Ok(b)) => call::decode::<IRateOracle::latestAnswerCall>(b)?,
        Some(Err(r)) => return Err(bad(format!("latestAnswer() оракула откатился: {r}"))),
        None => return Err(bad("нет ответа latestAnswer() оракула".into())),
    };
    // `convert(int256, uint256)` Vyper откатывает отрицательное; ноль — деление на курс в exchange (L468).
    if answer.is_negative() || answer.is_zero() {
        return Err(bad(format!(
            "курс оракула {answer} ≤ 0 — обмен пула откатится"
        )));
    }
    match load_curve_plain::state_with(raw, pair, wrapped, bad)? {
        PoolState::StableSwap(mut d) => {
            if d.rates.len() != 2 {
                return Err(bad(format!("у пула монет {}, а не 2", d.rates.len())));
            }
            if d.native != Some(0) {
                return Err(bad(
                    "монета 0 пула — не метка ETH 0xEeee… (не этот исходник)".into(),
                ));
            }
            d.rates[1] = answer.into_raw();
            d.pair_d = false;
            Ok(PoolState::StableSwap(d))
        }
        other => Ok(other),
    }
}
