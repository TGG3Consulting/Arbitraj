//! Файл: venues/bsc_dex/wallet/approve.rs
//! Что делает: разрешения на списание токена для обмена через Universal Router: ERC-20 approve → Permit2 своей
//! DEX и Permit2.approve(токен, Universal Router, сумма, срок). Читает текущие разрешения и возвращает только те
//! вызовы, которых не хватает. Сумма — на сделку или «без ограничения» (dex.approve_unlimited).
//! Кто вызывает: exec/plan.rs; котировка — через wallet/allow_cache.rs (read, lacking).
//! От чего зависит: node/call.rs, node/abi.rs, ctx.rs.
//! Настройки: dex.approve_unlimited, dex.permit2_expiration_s [bsc_dex].
//!
//! Источники: Permit2 — allowance(user, token, spender) → (uint160 amount, uint48 expiration, uint48 nonce),
//! approve(token, spender, uint160 amount, uint48 expiration), uint160.max — без ограничения:
//! https://github.com/Uniswap/permit2/blob/cc56ad0f3439c502c246fc5cfcc3db92bb8b7219/src/interfaces/IAllowanceTransfer.sol#L111-L123
//! (Permit2 PancakeSwap — тот же интерфейс); порядок для бота «approve на Permit2, затем permit2.approve на UR» —
//! developer.pancakeswap.finance, Infinity «Perform a swap» (research/flow/bsc_v4.md, раздел 7). ERC-20 — EIP-20.

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::{U48, U160};
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;

alloy::sol! {
    interface IPermit2 {
        function allowance(address user, address token, address spender) external view returns (uint160 amount, uint48 expiration, uint48 nonce);
        function approve(address token, address spender, uint160 amount, uint48 expiration) external;
    }
}

/// Вызов, который надо отправить (адрес контракта и данные).
pub type Approval = (Address, Bytes);

/// Текущие разрешения кошелька: ERC-20 allowance(кошелёк, Permit2), Permit2 allowance(кошелёк, токен, UR) — сумма и срок.
#[derive(Debug, Clone, Copy)]
pub struct Read {
    pub erc20: U256,
    pub permit2: U256,
    pub expiration: u64,
}

/// Читает разрешения с узла. Принимает: токен, кошелёк, Permit2, Universal Router. Ошибки: узел.
pub async fn read(
    ctx: &Ctx,
    token: Address,
    wallet: Address,
    permit2: Address,
    ur: Address,
) -> GatewayResult<Read> {
    let spender = permit2;
    let erc20 = call::view(
        ctx,
        token,
        &IERC20::allowanceCall {
            owner: wallet,
            spender,
        },
        "latest",
    )
    .await?;
    let user = wallet;
    let p = call::view(
        ctx,
        permit2,
        &IPermit2::allowanceCall {
            user,
            token,
            spender: ur,
        },
        "latest",
    )
    .await?;
    Ok(Read {
        erc20,
        permit2: U256::from(p.amount),
        expiration: p.expiration.to::<u64>(),
    })
}

/// Каких разрешений не хватает для списания amount токена Universal Router'ом через Permit2 (читает с узла).
/// Принимает: токен, кошелёк, Permit2, Universal Router, сумму, время блока (секунды).
/// Возвращает: вызовы по порядку (пусто — хватает). Ошибки: NotConfigured (настройки), узел.
pub async fn missing(
    ctx: &Ctx,
    token: Address,
    wallet: Address,
    permit2: Address,
    ur: Address,
    amount: U256,
    now: u64,
) -> GatewayResult<Vec<Approval>> {
    let r = read(ctx, token, wallet, permit2, ur).await?;
    lacking(ctx, &r, (token, permit2, ur), amount, now)
}

/// Каких разрешений не хватает при прочитанных `r`. Принимает: разрешения, (токен, Permit2, UR), сумму, время блока.
/// Ошибки: NotConfigured (настройки), BadResponse (см. permit2_approve).
pub fn lacking(
    ctx: &Ctx,
    r: &Read,
    (token, permit2, ur): (Address, Address, Address),
    amount: U256,
    now: u64,
) -> GatewayResult<Vec<Approval>> {
    let unlimited = ctx.flag("dex.approve_unlimited")?;
    let expiration = now + ctx.int("dex.permit2_expiration_s")?;
    let mut out = Vec::new();
    if r.erc20 < amount {
        out.push(erc20_approve(
            token,
            permit2,
            if unlimited { U256::MAX } else { amount },
        ));
    }
    if r.permit2 < amount || r.expiration <= now {
        out.push(permit2_approve(
            ctx, token, permit2, ur, amount, unlimited, expiration,
        )?);
    }
    Ok(out)
}

/// Все разрешения сразу (для пробного обмена в симуляции). Ошибки: NotConfigured, BadResponse (см. permit2_approve).
pub fn all(
    ctx: &Ctx,
    token: Address,
    permit2: Address,
    ur: Address,
    amount: U256,
    now: u64,
) -> GatewayResult<Vec<Approval>> {
    let unlimited = ctx.flag("dex.approve_unlimited")?;
    let expiration = now + ctx.int("dex.permit2_expiration_s")?;
    Ok(vec![
        erc20_approve(token, permit2, if unlimited { U256::MAX } else { amount }),
        permit2_approve(ctx, token, permit2, ur, amount, unlimited, expiration)?,
    ])
}

/// ERC-20 approve. Ошибок нет.
fn erc20_approve(token: Address, spender: Address, value: U256) -> Approval {
    (
        token,
        Bytes::from(IERC20::approveCall { spender, value }.abi_encode()),
    )
}

/// Permit2.approve: «без ограничения» (dex.approve_unlimited) — uint160.max, иначе ровно сумма сделки.
/// Ошибки: BadResponse — сумма больше uint160 (разрешение молча не расширяется до максимума);
/// NotConfigured — срок (время блока + dex.permit2_expiration_s) больше uint48.
fn permit2_approve(
    ctx: &Ctx,
    token: Address,
    permit2: Address,
    ur: Address,
    amount: U256,
    unlimited: bool,
    expiration: u64,
) -> GatewayResult<Approval> {
    let amount = if unlimited {
        U160::MAX
    } else {
        U160::checked_from_limbs_slice(amount.as_limbs()).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("Сумма разрешения {amount} больше uint160 Permit2"),
            )
        })?
    };
    let exp = U48::try_from(expiration).map_err(|_| {
        ctx.err(
            ErrorCode::NotConfigured,
            format!("Срок Permit2 {expiration} больше uint48 — проверьте dex.permit2_expiration_s"),
        )
    })?;
    let data = IPermit2::approveCall {
        token,
        spender: ur,
        amount,
        expiration: exp,
    }
    .abi_encode();
    Ok((permit2, Bytes::from(data)))
}
