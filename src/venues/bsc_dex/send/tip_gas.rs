//! Файл: venues/bsc_dex/send/tip_gas.rs
//! Что делает: газ перевода чаевых канала для ПЛАНА (Тигран 09.11: согласовано; решение 236, дополнение п. 6 — факт уже
//! считает газ этой транзакции по квитанции, exec/chain_tx.rs tip_gas). Чаевые — отдельная транзакция перевода BNB на
//! адрес канала (exec/txs.rs: tx::limit — eth_estimateGas от кошелька на адрес чаевых, пустые данные, сумма чаевых; та
//! же цена газа, что у обмена). План берёт её единицы газа той же оценкой узла (eth_estimateGas, без запаса
//! send.gas_limit_margin_pct: запас — лимит, а платится израсходованный газ), так же, как газ разрешений и обёртки —
//! единицы из данных узла (замеры проб), не из настройки. Оценка запоминается по (кошелёк, адрес чаевых, сумма) на время
//! процесса — на каждый тик котировки узел не спрашивается. Чаевые в плане есть (send::planned_tip: ровно один готовый
//! канал, бандлом, с чаевыми) — есть и их газ; нет — газа нет.
//! Кто вызывает: quote/mod.rs (котировка обмена), quote/cycle_cost.rs и exec/{cycle_exec, cycle_bundle_trade,
//! cycle_dry}.rs (цикл DEX ↔ DEX).
//! От чего зависит: send/mod.rs (tip — те же правила, что у отправки), send/{channels, ready}.rs, node/rpc.rs,
//! wallet/signer.rs, num.rs, ctx.rs.
//! Настройки: через ready.rs — channel.* (каналы и чаевые).
//!
//! eth_estimateGas — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/execute.yaml#L43-L60
//! (тот же вызов, что send/tx.rs limit).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

use alloy_primitives::{Address, U64, U256};
use rust_decimal::Decimal;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::num;
use super::super::wallet::signer;
use super::ready::Ready;

/// Ключ оценки: (кошелёк, адрес чаевых, сумма wei).
type Key = (Address, Address, U256);

/// Оценки узла: ключ → единицы газа перевода.
static UNITS: LazyLock<Mutex<HashMap<Key, u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Единицы газа перевода чаевых по оценке узла (память — шапка). Принимает: окружение. Возвращает: None — чаевых в
/// плане нет (send::planned_tip). Ошибки: ключ кошелька, узел (eth_estimateGas — текст узла: например, на кошельке
/// меньше BNB, чем чаевые; та же оценка при отправке откажет так же).
pub async fn planned_units(ctx: &Ctx) -> GatewayResult<Option<u64>> {
    let ready: Vec<Ready> = super::channels::ALL
        .iter()
        .filter_map(|c| super::ready::prepare(ctx, c).ok())
        .collect();
    let Some((to, amount)) = super::tip(&ready) else {
        return Ok(None);
    };
    let from = signer::address(ctx)?;
    let key = (from, to, amount);
    let known = memo().get(&key).copied();
    if let Some(u) = known {
        return Ok(Some(u));
    }
    let body = json!({ "from": from, "to": to, "input": "0x", "value": amount });
    let est: U64 = rpc::call(ctx, "eth_estimateGas", json!([body])).await?;
    let units = est.to::<u64>();
    memo().insert(key, units);
    Ok(Some(units))
}

/// Газ перевода чаевых в BNB при цене газа `wei` (та же цена, что у обмена: транзакция чаевых подписывается с ней —
/// exec/txs.rs). Принимает: окружение, цену газа. Возвращает: None — чаевых в плане нет. Ошибки: как planned_units;
/// BadResponse — число не переводится в BNB.
pub async fn planned_bnb(ctx: &Ctx, wei: U256) -> GatewayResult<Option<Decimal>> {
    let Some(units) = planned_units(ctx).await? else {
        return Ok(None);
    };
    let w = wei.saturating_mul(U256::from(units));
    num::to_decimal(w, num::BNB_DECIMALS)
        .map(Some)
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("Газ перевода чаевых {w} wei не переводится в BNB"),
            )
        })
}

/// Память оценок под замком. Ошибок нет.
fn memo() -> MutexGuard<'static, HashMap<Key, u64>> {
    UNITS.lock().unwrap_or_else(|p| p.into_inner())
}
