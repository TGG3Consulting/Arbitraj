//! Файл: venues/bsc_dex/pools/aero_mev.rs
//! Что делает: решение 470 (T451-01/02) — условие «MEV-налог Aero равен нулю» для наших обменов. Модуль
//! PriorityFeeMevTaxModule (Dromos-Labs/metadex-public 1.1.0, V3/src/fees/PriorityFeeMevTaxModule.sol L130-L146)
//! берёт налог только при `tx.gasprice − block.basefee > max(baseFeeFactor·basefee, minThreshold)` (L131, L136-L137).
//! Наша транзакция — тип 2 (dex.gas_mode = eip1559_l1): цена газа = min(maxFeePerGas, baseFee + чаевые) ≤ baseFee +
//! чаевые, значит превышение над basefee ≤ чаевых. Чаевые не выше minThreshold ⇒ превышение ≤ порога ⇒ налог 0. Это
//! Множитель priorityFeeMultiplier = 0 — налог 0 всегда (L139-L143). Правило проверяется по НАИБОЛЬШИМ чаевым режима (fixed — send.priority_fee_gwei, profit_share — send.tip_cap_gwei).
//! Не выполнено — пул не читается (ошибка с причиной): налог в котировке не учитывается, обмен через шаг SOLIDLY
//! исполнителя откатился бы (выход = getAmountOut без налога, проверка K пула — Pool.sol L172-L193), у пула Slipstream
//! V3 комиссия обмена была бы выше котировки.
//! Кто вызывает: pools/load_aero_basic.rs; exec/solidly_gate.rs (fresh_cap — перед отправкой, свежими с фабрики).
//! От чего зависит: node/call.rs, send/gas_mode.rs, send/tip_mode.rs, pools/abi_aero_basic.rs.
//! Настройки: dex.gas_mode, send.tip_mode, send.priority_fee_gwei, send.tip_cap_gwei [площадка DEX] (через
//! gas_mode.rs и tip_mode.rs).

use alloy_primitives::{Address, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::send::gas_mode::{self, GasMode};
use super::super::send::tip_mode::{self, TipMode};
use super::abi_aero_basic::{IAeroBasicFactory, IAeroMevTaxModule};

/// Наибольшие чаевые режима, wei за газ: fixed — сами чаевые, profit_share — верхний предел. Ошибок нет.
pub fn max_tip(m: &TipMode) -> u128 {
    match m {
        TipMode::Fixed(t) => *t,
        TipMode::ProfitShare(c) => c.cap,
    }
}

/// Правило «налог 0» по числам: чаевые ≤ minThreshold. Принимает: наибольшие чаевые и minThreshold, wei.
/// Возвращает: None — налога нет; Some(причина) — налог возможен. Ошибок нет.
pub fn tax_reason(tip: u128, min_threshold: U256) -> Option<String> {
    if U256::from(tip) <= min_threshold {
        return None;
    }
    Some(format!(
        "MEV-налог Aero: чаевые {tip} wei > minThreshold модуля {min_threshold} wei — обмен заплатил бы налог \
         (PriorityFeeMevTaxModule.sol L136-L143)"
    ))
}

/// Проверка модуля MEV-налога на блоке. Принимает: окружение, адрес модуля (нулевой — модуля нет, налога нет), тег
/// блока. Ошибки: NotApplicable — модель газа не eip1559_l1 (при legacy доля сверх basefee неизвестна) или чаевые выше
/// minThreshold; NotConfigured — не заданы dex.gas_mode / режим чаевых; узел и ответ не по ABI (у модуля нет
/// minThreshold — не PriorityFeeMevTaxModule).
pub async fn check(ctx: &Ctx, module: Address, tag: &str) -> GatewayResult<()> {
    if module.is_zero() {
        return Ok(());
    }
    if gas_mode::read(ctx)? != GasMode::Eip1559L1 {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "MEV-налог Aero: нужна dex.gas_mode = eip1559_l1 (при legacy превышение цены газа над basefee неизвестно)",
        ));
    }
    // Множитель 0 — налог 0 при любых чаевых: mulDivUp(…, 0, PRECISION) = 0 (L139-L143; так у модуля CL Arbitrum по
    // аргументам конструктора clMevTaxModuleConstructorArgs).
    let ask = IAeroMevTaxModule::priorityFeeMultiplierCall {};
    let mult = call::view(ctx, module, &ask, tag).await?;
    if mult == 0 {
        return Ok(());
    }
    let tip = max_tip(&tip_mode::read(ctx)?);
    let min = call::view(ctx, module, &IAeroMevTaxModule::minThresholdCall {}, tag).await?;
    match tax_reason(tip, U256::from(min.to::<u128>())) {
        None => Ok(()),
        Some(w) => Err(ctx.err(ErrorCode::NotApplicable, w)),
    }
}

/// Перед отправкой (решение 470): пауза фабрики Aero Basic и правило налога её текущего модуля на последнем блоке.
/// Принимает: окружение, фабрика. Ошибки: NoRoute — фабрика на паузе (Pool.sol L144); как check; узел.
pub async fn fresh(ctx: &Ctx, factory: Address) -> GatewayResult<()> {
    if call::view(ctx, factory, &IAeroBasicFactory::isPausedCall {}, "latest").await? {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "фабрика Aero Basic на паузе (isPaused) — обмен откатился бы",
        ));
    }
    let ask = IAeroBasicFactory::mevTaxModuleCall {};
    let module = call::view(ctx, factory, &ask, "latest").await?;
    check(ctx, module, "latest").await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Граница правила: равно — налога нет (L137 `<=`), на 1 wei больше — налог возможен.
    #[test]
    fn boundary() {
        assert!(tax_reason(1_000_000_000, U256::from(1_000_000_000u64)).is_none());
        assert!(tax_reason(1_000_000_001, U256::from(1_000_000_000u64)).is_some());
        assert!(tax_reason(0, U256::ZERO).is_none());
    }
}
