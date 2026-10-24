//! Файл: venues/bsc_dex/check/probe_calls.rs
//! Что делает: вызовы пробного обмена для eth_simulateV1 — переводы, разрешения и балансы ERC-20.
//! Кто вызывает: check/probe.rs.
//! Там же: где лежат токены пула (holder), прошла ли продажа (sell_ok), откат шага пробной покупки — вердикт по монете или помеха (buy_revert; срок транзакции вышел — помеха).
//! От чего зависит: check/simulate.rs (Call), node/abi.rs, quote/quoter_revert.rs.
//! Настройки: нет.
//!
//! ERC-20 — https://eips.ethereum.org/EIPS/eip-20 .

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolKind, PoolRef};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::ur;
use super::super::node::abi::IERC20;
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
use super::super::quote::quoter_revert;
use super::simulate::{Call, CallResult};

/// Вызов from → to с данными. Ошибок нет.
pub fn call(from: Address, to: Address, data: Vec<u8>) -> Call {
    Call {
        from,
        to,
        input: Bytes::from(data),
    }
}

/// transfer токена. Ошибок нет.
pub fn transfer(from: Address, token: Address, to: Address, value: U256) -> Call {
    call(from, token, IERC20::transferCall { to, value }.abi_encode())
}

/// approve токена. Ошибок нет.
pub fn approve(from: Address, token: Address, spender: Address, value: U256) -> Call {
    call(
        from,
        token,
        IERC20::approveCall { spender, value }.abi_encode(),
    )
}

/// balanceOf. Ошибок нет.
pub fn balance(from: Address, token: Address, owner: Address) -> Call {
    call(from, token, IERC20::balanceOfCall { owner }.abi_encode())
}

/// Откат шага пробной покупки: вердикт по монете или помеха окружения. Шаги до квотера (перевод USDT из пула-источника,
/// разрешения на USDT) монету не касаются; откат квотера «не хватает ликвидности» (quote/quoter_revert.rs) — свойство
/// пула на эту сумму. Это — Err NoRoute: проверка не выполнена, монета не в чёрный список.
/// Принимает: номер шага, итог, номер шага квотера. Возвращает: Ok(причина вердикта) или Err.
pub fn buy_revert(ctx: &Ctx, i: usize, r: &CallResult, quoter: usize) -> GatewayResult<String> {
    let why = format!("шаг {i} пробной покупки откатился: {}", r.why());
    let msg = r
        .error
        .as_ref()
        .map(|e| e.message.as_str())
        .unwrap_or_default();
    // Срок транзакции вышел (TransactionDeadlinePassed) — не свойство монеты: не проверено, повтор (не чёрный список).
    if late(r) {
        let m = format!(
            "{why} — TransactionDeadlinePassed: срок транзакции меньше шага времени симулированного блока (у BSC +12 с к базовому, check/simulate.rs) — увеличьте dex.deadline_s"
        );
        return Err(ctx.err(ErrorCode::NoRoute, m));
    }
    let env =
        i < quoter || (i == quoter && quoter_revert::classify(msg, Some(&r.return_data)).is_some());
    if env {
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    Ok(why)
}

/// Откат «срок транзакции вышел» (TransactionDeadlinePassed): не вердикт по монете — срок меньше шага времени
/// симулированного блока (настройка), проверка «не проверено».
/// Принимает: результат вызова. Ошибок нет.
pub fn late(r: &CallResult) -> bool {
    let msg = r
        .error
        .as_ref()
        .map(|e| e.message.as_str())
        .unwrap_or_default();
    !r.ok() && ur::deadline_passed(&r.return_data, msg)
}

/// uint256 из ответа balanceOf. None — не по ABI.
pub fn u256(r: &CallResult) -> Option<U256> {
    IERC20::balanceOfCall::abi_decode_returns(&r.return_data).ok()
}

/// Продажа пробы прошла (последний вызов). Ошибок нет.
pub fn sell_ok(all: &[CallResult]) -> bool {
    all.last().is_some_and(|c| c.ok())
}

/// Адрес, где лежат токены пула: V2/V3 — сам пул; Infinity — Vault (dex.vault); Uniswap V4 — PoolManager.
/// Ошибки: NotConfigured (dex.vault).
pub fn holder(ctx: &Ctx, e: &Entry) -> GatewayResult<Address> {
    Ok(match (&e.pool.reference, e.pool.kind, e.family.proto) {
        (PoolRef::Contract(a), PoolKind::V2 | PoolKind::V3, _) => *a,
        (_, _, Proto::InfinityCl | Proto::InfinityBin) => ctx.dex_addr("dex.vault", e.family.id)?,
        (PoolRef::Singleton { manager, .. }, _, _) => *manager,
        (PoolRef::Contract(a), _, _) => *a,
    })
}
