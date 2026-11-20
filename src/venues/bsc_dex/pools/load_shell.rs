//! Файл: venues/bsc_dex/pools/load_shell.rs
//! Что делает: решение 486 — чтение состояния пула Shell Protocol (Proteus) на блоке двумя try_many: (1) пул —
//! ocean(), xToken(), yToken(), implementation(), poolFrozen(); (2) учёт — balanceOfBatch([пул, пул], [x, y]) и
//! unwrapFeeDivisor(), реализация — getSlopes / getAs / getBs / getKs / BASE_FEE, токены ERC-20 сторон — decimals().
//! Учётный контракт пула должен быть dex.vault DEX; формула полезности — по спискам сверенных реализаций
//! (shell_sides.rs). Собирает PoolState::Proteus. Балансы меняют только обмены / внесения / снятия через учёт (события
//! пула), реализацию и заморозку — события пула, комиссию вывода — событие учёта; вид ещё перечитывается каждым блоком
//! (per_block.rs) — комиссию вывода и чужие изменения не пропустить.
//! Кто вызывает: pools/load.rs (ветка Proto::ShellProteus).
//! От чего зависит: ctx.rs, node/call.rs, abi_shell.rs, shell_sides.rs, stable_native.rs (wrapped).
//! Настройки: dex.vault, dex.proteus_utility_v1 / _v2 [DEX], dex.native_wrapped [bsc_dex].
//!
//! Источник: Sourcify 42161 LiquidityPool.sol (v3 L29-L36 ocean / xToken / yToken, L271-L281 _getBalances —
//! balanceOfBatch учёта), LiquidityPoolProxy.sol L16-L17 (implementation, poolFrozen), Slices.sol (getSlopes / getAs /
//! getBs / getKs), Proteus.sol BASE_FEE; Ocean.sol unwrapFeeDivisor (aaa0ccf L96; v3 L110), _erc20Unwrap — decimals()
//! токена (v3 L727).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, I256, U256};

use crate::core::types::{PoolState, ProteusData, ProteusUtility};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_shell::{IProteus as R, IShellOcean as O, IShellPool as P, IShellToken as T};
use super::shell_sides::{self, Side};
use super::stable_native;

fn enc<C: SolCall>(c: C) -> Bytes {
    Bytes::from(c.abi_encode())
}

/// Вызовы первого шага: поля пула. Ошибок нет.
pub fn calls1(pool: Address) -> Vec<(Address, Bytes)> {
    vec![
        (pool, enc(P::oceanCall {})),
        (pool, enc(P::xTokenCall {})),
        (pool, enc(P::yTokenCall {})),
        (pool, enc(P::implementationCall {})),
        (pool, enc(P::poolFrozenCall {})),
    ]
}

/// Поля пула (ответы первого шага).
#[derive(Debug, Clone)]
pub struct Head {
    pub ocean: Address,
    pub x_id: U256,
    pub y_id: U256,
    pub implementation: Address,
    pub frozen: bool,
}

/// Вызовы второго шага: балансы и комиссия вывода учёта, параметры реализации, знаки токенов ERC-20 сторон (у нативной
/// стороны — нет вызова). Ошибок нет.
pub fn calls2(pool: Address, h: &Head, x: Side, y: Side) -> Vec<(Address, Bytes)> {
    let batch = O::balanceOfBatchCall {
        accounts: vec![pool, pool],
        ids: vec![h.x_id, h.y_id],
    };
    let mut v = vec![
        (h.ocean, enc(batch)),
        (h.ocean, enc(O::unwrapFeeDivisorCall {})),
        (h.implementation, enc(R::getSlopesCall {})),
        (h.implementation, enc(R::getAsCall {})),
        (h.implementation, enc(R::getBsCall {})),
        (h.implementation, enc(R::getKsCall {})),
        (h.implementation, enc(R::BASE_FEECall {})),
    ];
    for (t, native) in [x, y] {
        if !native {
            v.push((t, enc(T::decimalsCall {})));
        }
    }
    v
}

/// Ответы второго шага (разобранные).
#[derive(Debug, Clone)]
pub struct Body {
    pub balances: Vec<U256>,
    pub divisor: U256,
    pub slopes: Vec<i128>,
    pub a: Vec<i128>,
    pub b: Vec<i128>,
    pub k: Vec<i128>,
    pub base_fee: U256,
    /// decimals() ERC-20 сторон x и y (у нативной — None).
    pub dec_x: Option<u8>,
    pub dec_y: Option<u8>,
}

/// Состояние пула на блоке. Принимает: окружение, id DEX, пул, пару графа, блок. Ошибки: BadResponse (откат, не по
/// ABI, учёт не dex.vault, стороны не из пары, реализация не сверена, параметры срезов не сходятся), NotConfigured
/// (dex.vault).
pub async fn load(
    ctx: &Ctx,
    dex: &str,
    pool: Address,
    pair: (Address, Address),
    block: u64,
) -> GatewayResult<PoolState> {
    let bad = |what: String| ctx.err(ErrorCode::BadResponse, format!("Shell {pool:#x}: {what}"));
    let tag = call::block_tag(block);
    let l1 = calls1(pool);
    let got = call::try_many(ctx, &l1, &tag).await?;
    let ok = |g: &[Result<Bytes, String>], i: usize| -> GatewayResult<Bytes> {
        match g.get(i) {
            Some(Ok(b)) => Ok(b.clone()),
            Some(Err(r)) => Err(bad(format!("вызов {i} откатился: {r}"))),
            None => Err(bad(format!("нет ответа на вызов {i}"))),
        }
    };
    let head = Head {
        ocean: call::decode::<P::oceanCall>(&ok(&got, 0)?)?,
        x_id: call::decode::<P::xTokenCall>(&ok(&got, 1)?)?,
        y_id: call::decode::<P::yTokenCall>(&ok(&got, 2)?)?,
        implementation: call::decode::<P::implementationCall>(&ok(&got, 3)?)?,
        frozen: call::decode::<P::poolFrozenCall>(&ok(&got, 4)?)?,
    };
    let vault = ctx.dex_addr("dex.vault", dex)?;
    if head.ocean != vault {
        return Err(bad(format!("учёт пула {:#x} ≠ dex.vault {vault:#x}", head.ocean)));
    }
    let (x, y) = shell_sides::sides(head.x_id, head.y_id, pair, stable_native::wrapped(ctx))
        .map_err(&bad)?;
    let utility = shell_sides::utility(ctx, dex, head.implementation).map_err(&bad)?;
    let l2 = calls2(pool, &head, x, y);
    let got = call::try_many(ctx, &l2, &tag).await?;
    let mut n = 7;
    let mut dec = |native: bool| -> GatewayResult<Option<u8>> {
        if native {
            return Ok(None);
        }
        n += 1;
        Ok(Some(call::decode::<T::decimalsCall>(&ok(&got, n - 1)?)?))
    };
    let (dec_x, dec_y) = (dec(x.1)?, dec(y.1)?);
    let body = Body {
        balances: call::decode::<O::balanceOfBatchCall>(&ok(&got, 0)?)?,
        divisor: call::decode::<O::unwrapFeeDivisorCall>(&ok(&got, 1)?)?,
        slopes: call::decode::<R::getSlopesCall>(&ok(&got, 2)?)?,
        a: call::decode::<R::getAsCall>(&ok(&got, 3)?)?,
        b: call::decode::<R::getBsCall>(&ok(&got, 4)?)?,
        k: call::decode::<R::getKsCall>(&ok(&got, 5)?)?,
        base_fee: call::decode::<R::BASE_FEECall>(&ok(&got, 6)?)?,
        dec_x,
        dec_y,
    };
    state(&head, x, y, utility, &body).map_err(bad)
}

/// Сборка состояния из прочитанного (без сети; её же зовут тесты на записанных ответах). Принимает: поля пула, стороны,
/// формулу, ответы второго шага. Возвращает: PoolState::Proteus или причину (балансов не два, длины срезов не
/// сходятся, нет знаков ERC-20 стороны).
pub fn state(
    h: &Head,
    x: Side,
    y: Side,
    utility: ProteusUtility,
    b: &Body,
) -> Result<PoolState, String> {
    let [xb, yb] = b.balances[..] else {
        return Err(format!("balanceOfBatch вернул {} балансов, нужно 2", b.balances.len()));
    };
    let n = b.a.len();
    if n == 0 || b.b.len() != n || b.k.len() != n || b.slopes.len() + 1 != n {
        return Err(format!(
            "срезов a / b / k = {} / {} / {}, наклонов {} — не сходится",
            n,
            b.b.len(),
            b.k.len(),
            b.slopes.len()
        ));
    }
    let dec = |s: Side, d: Option<u8>| if s.1 { Some(18) } else { d };
    let conv = |v: &[i128]| v.iter().map(|q| int128(*q)).collect::<Vec<_>>();
    Ok(PoolState::Proteus(Box::new(ProteusData {
        ocean: h.ocean,
        implementation: h.implementation,
        x_id: h.x_id,
        y_id: h.y_id,
        token_x: x.0,
        token_y: y.0,
        native_x: x.1,
        native_y: y.1,
        dec_x: dec(x, b.dec_x).ok_or("нет decimals() токена стороны x")?,
        dec_y: dec(y, b.dec_y).ok_or("нет decimals() токена стороны y")?,
        x_bal: xb,
        y_bal: yb,
        slopes: conv(&b.slopes),
        a: conv(&b.a),
        b: conv(&b.b),
        k: conv(&b.k),
        base_fee: b.base_fee,
        utility,
        unwrap_fee_divisor: b.divisor,
        frozen: h.frozen,
    })))
}

/// int128 ABDK → I256 (дополнительный код). Ошибок нет.
pub fn int128(q: i128) -> I256 {
    let u = U256::from(q.unsigned_abs());
    I256::from_raw(if q < 0 { u.wrapping_neg() } else { u })
}
