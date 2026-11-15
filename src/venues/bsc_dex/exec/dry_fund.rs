//! Файл: venues/bsc_dex/exec/dry_fund.rs
//! Что делает: вход сухой сделки DEX (решение 121, C4), когда у кошелька его не хватает: тот же приём, что у пробы
//! налогов (check/probe.rs, check/service_tax_calls.rs) — первым вызовом той же симуляции перевод токена на кошелёк от
//! имени пула-источника (eth_simulateV1 с validation = false не требует подписи отправителя), без state override.
//! Источник — пул V2/V3 реестра (адрес контракта держит свои токены), достоверный, НЕ из маршрута сделки (иначе
//! перевод сдвинул бы его баланс против резервов), с наибольшим балансом этого токена ≥ суммы. Сумма перевода —
//! недостача, увеличенная на налог токена (наибольший из налога покупки и перевода — перевод от пула токен может
//! считать покупкой), чтобы после налога у кошелька было не меньше входа; что реально дошло — меряет симуляция.
//! Решение 302 (задача t133): пула-источника нет и задана presell.fund_fallback = singleton — запасной источник,
//! держатель-синглтон Uniswap V4 PoolManager / Infinity Vault (exec/dry_fund_holder.rs, там же — почему это честно и
//! когда держатель не берётся). Не задана — как до решения (тот же отказ тем же текстом).
//! Кто вызывает: exec/dry_sim.rs (plan).
//! От чего зависит: pools/registry.rs, node/call.rs (view_many: Multicall3 / пачка), node/abi.rs (IERC20),
//! exec/dry_build.rs (entries — пулы маршрута), exec/dry_fund_holder.rs.
//! Настройки: presell.fund_fallback [bsc_dex] (через dry_fund_holder; через вызываемые — dex.multicall3, dex.vault).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolRef, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::num;
use super::super::quote::Quoting;
use super::dry_build::{Ready, entries};
use super::dry_fund_holder;

/// Пополнение: откуда (адрес), сколько, что это за источник (для журнала: «пула» или «держателя-синглтона (…)»).
pub struct Fund {
    pub src: Address,
    pub amount: U256,
    pub what: String,
}

/// Пополнение кошелька в симуляции: источник и сумма перевода. Принимает: котировку, сделку (пулы маршрута
/// исключаются), токен, (есть у кошелька, нужно), налоги токена. Возвращает: Ok(Ok(Fund)) или
/// Ok(Err(причина)) — пополнение невозможно (налог ≥ 100 %, источника нет). Ошибки: узел (balanceOf), Internal
/// (настройки).
pub async fn plan(
    q: &Quoting,
    r: &Ready,
    token: Address,
    (have, need): (U256, U256),
    taxes: Option<&TokenTaxes>,
) -> GatewayResult<Result<Fund, String>> {
    let legs = entries(&r.legs);
    let route: Vec<_> = legs.iter().map(|e| e.pool.reference.clone()).collect();
    let Some(amount) = gross(need.saturating_sub(have), taxes) else {
        return Ok(Err(
            "налог токена входа не позволяет посчитать пополнение".into()
        ));
    };
    if let Some(src) = source(q, token, &route, amount).await? {
        return Ok(Ok(Fund {
            src,
            amount,
            what: "пула".into(),
        }));
    }
    let none = format!(
        "у кошелька {have} единиц входа, нужно {need}; пула-источника (V2/V3 вне маршрута) с балансом ≥ {amount} нет — проверьте на сумме не больше баланса"
    );
    // Решение 302: запасной источник — только при заданной presell.fund_fallback.
    if dry_fund_holder::fallback(&q.ctx)?.is_none() {
        return Ok(Err(none));
    }
    Ok(
        match dry_fund_holder::pick(q, token, &legs, amount).await? {
            Ok((src, what)) => Ok(Fund { src, amount, what }),
            Err(why) => Err(format!(
                "{none}; запасной источник ({} = {}) не взят: {why}",
                dry_fund_holder::SETTING,
                dry_fund_holder::SINGLETON
            )),
        },
    )
}

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
            PoolRef::Singleton { .. } | PoolRef::Member { .. } | PoolRef::Multi { .. } => None,
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
