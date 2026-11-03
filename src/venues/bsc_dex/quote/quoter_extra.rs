//! Файл: venues/bsc_dex/quote/quoter_extra.rs
//! Что делает: ДОПОЛНИТЕЛЬНЫЙ квотер сверки (решение «122 (Infinity Stable, ответы)», Тигран): у пула PancakeSwap
//! Infinity Stable, кроме основного CLQuoter Infinity (настоящий swap с крючком, quote/quoter_calls.rs), выход
//! сверяется ещё и с квотером самого крючка get_dy(int128 i, int128 j, uint256 dx). Оба вызова идут ПАРАЛЛЕЛЬНО (по
//! одному eth_call на свой узел, друг друга не ждут — вызывающий соединяет их join). Расхождение нашего расчёта с
//! любым больше dex.quoter_max_diff_pct, откат или сбой любого — сделки нет, с причиной. У других видов пулов
//! дополнительного квотера нет (None).
//! Кто вызывает: quote/quoter.rs (compare — сверка решения 73 и перед сделкой), quote/mixed_quoter.rs (сверка умного
//! маршрута: основной — MixedQuoter с действием INFI_CL, дополнительный — get_dy шага).
//! От чего зависит: node/rpc.rs, pools/abi_inf_stable.rs, pools/registry.rs, quote/quoter.rs (within).
//! Настройки: dex.quoter_max_diff_pct [bsc_dex].
//!
//! Источник: get_dy крючка — verified-код CLStableSwapHook 0x86c3DC08… L1188-L1199 (считает views-контракт фабрики);
//! индексы: монета currency0 — 0, currency1 — 1 (beforeInitialize L1654, beforeSwap L1695-L1696).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde_json::json;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::pools::abi_inf_stable::IInfStableHook as H;
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;

/// Есть ли у пула дополнительный квотер. Ошибок нет.
pub fn has(e: &Entry) -> bool {
    e.family.proto == Proto::InfinityStable
}

/// Вызов get_dy крючка на точный вход. None — у пула дополнительного квотера нет. Ошибок нет.
pub fn request(e: &Entry, token_in: Address, amount: U256) -> Option<(Address, Bytes)> {
    if !has(e) {
        return None;
    }
    let PoolRef::Contract(hook) = e.pool.reference else {
        return None;
    };
    let (i, j) = if token_in == e.pool.token0 {
        (0i128, 1i128)
    } else {
        (1, 0)
    };
    let data = H::get_dyCall { i, j, dx: amount }.abi_encode();
    Some((hook, Bytes::from(data)))
}

/// Сверка нашего выхода с get_dy крючка. Принимает: запись, токен входа, сумму, наш выход. Возвращает: Ok(()) — у
/// пула нет дополнительного квотера или расхождение в допуске; Err(причина) — расхождение, откат, сбой узла, ответ не
/// по ABI (любое — сделки нет). Ошибки: NotConfigured (dex.quoter_max_diff_pct).
pub async fn check(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    amount: U256,
    local: U256,
) -> GatewayResult<Result<(), String>> {
    let Some((to, data)) = request(e, token_in, amount) else {
        return Ok(Ok(()));
    };
    let raw: Bytes = match rpc::call(
        ctx,
        "eth_call",
        json!([{ "to": to, "input": data }, "latest"]),
    )
    .await
    {
        Ok(r) => r,
        Err(err) => {
            return Ok(Err(format!(
                "квотер крючка get_dy не ответил (откат или сбой узла): {}",
                err.message
            )));
        }
    };
    let Ok(remote) = H::get_dyCall::abi_decode_returns(&raw) else {
        return Ok(Err("ответ get_dy крючка не по ABI".into()));
    };
    Ok(super::quoter::within(ctx, local, remote, Decimal::ZERO)?
        .map_err(|why| format!("квотер крючка get_dy: {why}")))
}
