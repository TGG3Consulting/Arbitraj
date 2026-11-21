//! Файл: venues/bsc_dex/pools/load_curve_meta.rs
//! Что делает: решение 472 (доделка) — чтение метапула фабрики Curve Sidechain/L2 (вариант CurveMeta вида
//! curve_stableswap_meta: MetaUSD / MetaUSDBalances над 2pool, MetaFraxUSD / MetaFraxUSDBalances над fraxbp) на блоке.
//! Метапул — две монеты: своя монета (coins(0)) и LP-токен базового пула (coins(1)); `exchange(i, j)` между ними
//! считает пул формулой Plain2Basic с курсами `rates = [rate_multiplier, BASE_POOL.get_virtual_price()]` —
//! виртуальная цена базового пула читается в САМОМ обмене (MetaUSD.vy L623), кеша нет. Поэтому чтение — то же,
//! что у plain (load_curve_plain.rs: coins / balances / A_precise / fee / ma_exp_time), плюс последним вызовом той же
//! пачки — `get_virtual_price()` базового пула; курс монеты 1 = ответ, курс монеты 0 = 10^(36 − decimals)
//! (FactorySidechains.vy fb61207 deploy_metapool L645: `initialize(…, 10 ** (36 - decimals), …)`). D_P — циклом
//! (get_D метапула L300-L334: `for x in _xp: D_P = D_P * D / (x * N_COINS)`), а не одной строкой, как у Plain2Basic.
//! Базовый пул — адрес, подтверждённый проверкой (verify_curve_plain.rs: get_base_pool(pool) фабрики = coins(1)
//! метапула), — хранится в памяти по (площадка, пул) до перезапуска; нет в памяти — пул не читается (не гадаем адрес).
//! Обмен с базовыми монетами (exchange_underlying) не используется: пара метапула — монета и LP-токен.
//! Кто вызывает: pools/load.rs, pools/load_site.rs (вариант CurveMeta), pools/verify_curve_plain.rs (remember).
//! От чего зависит: ctx.rs, node/call.rs (try_many), tokens.rs, load_curve.rs (Raw), load_curve_plain.rs,
//! abi_curve_plain.rs, gateway/shard_map.rs.
//! Настройки: нет (dex.multicall3 — через node/call.rs).
//!
//! Источник: curvefi/curve-factory f389123 contracts/implementations/meta/MetaUSD.vy (Vyper 0.2.15): rate_multiplier
//! L123, `_A()` L251 (тот же текст, что Plain2Basic), get_D L300-L334 (`raise` при несходимости), get_y L476-L519
//! (тот же текст, что Plain2Basic), get_dy L524-L540, exchange L606-L673 (L623: `rates = [self.rate_multiplier,
//! Curve(BASE_POOL).get_virtual_price()]`). MetaUSDBalances.vy того же коммита — balances(i) = balanceOf −
//! admin_balances (L261-L268), формула та же. В исходнике репозитория BASE_POOL — 3pool Ethereum (константа
//! шаблона); на Arbitrum развёрнуты реализации с базовым пулом 2pool / fraxbp (адреса — curve-js ab3f1c2
//! src/constants/factory/stable.ts L159-L166, базовый пул пула — get_base_pool фабрики): текст развёрнутого кода
//! здесь не сверен (Sourcify недоступен из среды) — сверка формулы — запись (get_dy = наш выход,
//! tests/arbitrum_curve_plain_live.rs).

use std::sync::LazyLock;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::shard_map::ShardMap;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::tokens::Tokens;
use super::abi_curve_plain::ICurveMeta;
use super::load_curve::Raw;
use super::load_curve_plain;

/// Базовый пул метапула по (площадка DEX, метапул) — из проверки (verify_curve_plain.rs).
static BASES: LazyLock<ShardMap<(&'static str, Address), Address>> =
    LazyLock::new(ShardMap::default);

/// Запомнить базовый пул метапула (проверка подтвердила его фабрикой). Ошибок нет.
pub fn remember(venue: &'static str, pool: Address, base: Address) {
    BASES.insert((venue, pool), base);
}

/// Базовый пул метапула из памяти. None — пул ещё не проверялся в этом запуске. Ошибок нет.
pub fn base(venue: &'static str, pool: Address) -> Option<Address> {
    BASES.get(&(venue, pool))
}

/// Вызовы чтения: вызовы plain (load_curve_plain::calls) и последним — get_virtual_price() базового пула.
/// Возвращает: вызовы и индекс A_precise(). Ошибок нет.
pub fn calls(pool: Address, base: Address) -> (Vec<(Address, Bytes)>, usize) {
    let (mut calls, tail) = load_curve_plain::calls(pool);
    calls.push((
        base,
        ICurveMeta::get_virtual_priceCall {}.abi_encode().into(),
    ));
    (calls, tail)
}

/// Сетевое чтение метапула без пары: вызовы calls и decimals монет. Принимает: окружение, токены, метапул,
/// блок-тег. Ошибки: базовый пул не подтверждён проверкой, узел, ответов не столько, сколько вызовов, ответ не по ABI.
pub async fn read(ctx: &Ctx, tokens: &Tokens, pool: Address, tag: &str) -> GatewayResult<Raw> {
    let Some(base) = base(ctx.place.id, pool) else {
        let what =
            format!("метапул Curve {pool}: базовый пул не подтверждён проверкой в этом запуске");
        return Err(ctx.err(ErrorCode::BadResponse, what));
    };
    let (calls, tail) = calls(pool, base);
    let got = call::try_many(ctx, &calls, tag).await?;
    if got.len() != calls.len() {
        let what = format!(
            "метапул Curve {pool}: ответов {} из {}",
            got.len(),
            calls.len()
        );
        return Err(ctx.err(ErrorCode::BadResponse, what));
    }
    let mut decimals = Vec::new();
    for k in 0..2 {
        let Some(Ok(c)) = got.get(2 * k) else { break };
        let coin = call::decode::<super::abi_curve::IStableSwapV1::coinsCall>(c)?;
        decimals.push(tokens.decimals(ctx, coin).await?);
    }
    Ok(Raw {
        pool,
        got,
        tail,
        decimals,
    })
}

/// Состояние пары метапула на блоке. Принимает: окружение, токены, метапул, пару, блок-тег. Ошибки: как у read и
/// state.
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
    state_with(raw, pair, &|what: String| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("метапул Curve {pool}: {what}"),
        )
    })
}

/// Состояние plain (load_curve_plain::state_with, обёртки нет — у метапула нет нативной монеты) с курсом монеты 1 =
/// get_virtual_price() базового пула и D_P циклом. Ошибки: как у load_curve_plain::state_with; монет не две;
/// get_virtual_price откатился или не по ABI.
pub fn state_with(
    raw: &Raw,
    pair: (Address, Address),
    bad: &dyn Fn(String) -> GatewayError,
) -> GatewayResult<PoolState> {
    let vp = match raw.got.last() {
        Some(Ok(b)) => call::decode::<ICurveMeta::get_virtual_priceCall>(b)?,
        Some(Err(r)) => {
            return Err(bad(format!(
                "get_virtual_price() базового пула откатился: {r}"
            )));
        }
        None => return Err(bad("нет ответа get_virtual_price() базового пула".into())),
    };
    match load_curve_plain::state_with(raw, pair, None, bad)? {
        PoolState::StableSwap(mut d) => {
            if d.rates.len() != 2 {
                return Err(bad(format!("у метапула монет {}, а не 2", d.rates.len())));
            }
            d.rates[1] = vp;
            d.pair_d = false;
            Ok(PoolState::StableSwap(d))
        }
        other => Ok(other),
    }
}
