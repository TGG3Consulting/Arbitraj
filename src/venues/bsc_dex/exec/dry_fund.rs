//! Файл: venues/bsc_dex/exec/dry_fund.rs
//! Что делает: вход сухой сделки DEX (решение 121, C4), когда у кошелька его не хватает: тот же приём, что у пробы
//! налогов (check/probe.rs, check/service_tax_calls.rs) — первым вызовом той же симуляции перевод токена на кошелёк от
//! имени пула-источника (eth_simulateV1 с validation = false не требует подписи отправителя), без state override.
//! Источник — пул V2/V3 реестра (адрес контракта держит свои токены), достоверный, НЕ из маршрута сделки (иначе
//! перевод сдвинул бы его баланс против резервов), с наибольшим балансом этого токена ≥ суммы. Сумма перевода —
//! недостача, увеличенная на налог токена (наибольший из налога покупки и перевода — перевод от пула токен может
//! считать покупкой), чтобы после налога у кошелька было не меньше входа; что реально дошло — меряет симуляция.
//! Кто вызывает: exec/dry_sim.rs.
//! От чего зависит: pools/registry.rs, node/call.rs (view_many: Multicall3 / пачка), node/abi.rs (IERC20), wallet/balances.rs.
//! Настройки: нет (через вызываемые — dex.multicall3).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolRef, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::num;
use super::super::quote::Quoting;

/// Пул-источник токена для перевода в симуляции. Принимает: котировку (реестр), токен, пулы маршрута (исключаются),
/// нужную сумму (минимальные единицы). Возвращает: адрес пула или None — подходящего нет. Ошибки: узел (balanceOf).
pub async fn source(
    q: &Quoting,
    token: Address,
    route: &[PoolRef],
    need: U256,
) -> GatewayResult<Option<Address>> {
    let pools: Vec<Address> = q
        .reg
        .all()
        .into_iter()
        .filter(|e| e.valid && matches!(e.pool.kind, PoolKind::V2 | PoolKind::V3))
        .filter(|e| e.pool.token0 == token || e.pool.token1 == token)
        .filter(|e| !route.contains(&e.pool.reference))
        .filter_map(|e| match e.pool.reference {
            PoolRef::Contract(a) => Some(a),
            PoolRef::Singleton { .. } | PoolRef::Member { .. } => None,
        })
        .collect();
    if pools.is_empty() {
        return Ok(None);
    }
    let calls: Vec<_> = pools
        .iter()
        .map(|a| IERC20::balanceOfCall { owner: *a })
        .collect();
    let balances = call::view_many(&q.ctx, token, &calls, "latest").await?;
    Ok(pools
        .into_iter()
        .zip(balances)
        .filter(|(_, b)| *b >= need)
        .max_by_key(|(_, b)| *b)
        .map(|(a, _)| a))
}

/// Сумма перевода с учётом налога токена: недостача / (1 − налог), с округлением вверх. Принимает: недостачу, налоги
/// токена (None — без налога: USDT, промежуточная). Возвращает: сумму; None — налог ≥ 100 % или не считается.
/// Ошибок нет.
pub fn gross(deficit: U256, taxes: Option<&TokenTaxes>) -> Option<U256> {
    let pct = taxes.map_or(Decimal::ZERO, |t| t.buy_pct.max(t.transfer_pct));
    if pct.is_zero() {
        return Some(deficit);
    }
    let keep = Decimal::ONE - pct / Decimal::ONE_HUNDRED;
    if keep <= Decimal::ZERO {
        return None;
    }
    // Вычисление в целых единицах токена через Decimal (decimals 0): деление с округлением вверх.
    let d = num::to_decimal(deficit, 0)?;
    num::to_units((d / keep).ceil(), 0)
}
