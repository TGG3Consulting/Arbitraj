//! Файл: venues/bsc_dex/wallet/transfer_cost.rs
//! Что делает: стоимость «вывода» DEX (перевода монеты из кошелька на биржу) для расчёта сделки: газ (пробный перевод
//! × цена газа) и налог на перевод (% проверки монеты). Нативная монета BNB (решение 62): газ — пробный нативный
//! перевод (check/native_probe.rs), налога нет — нативный перевод не вызывает контракт токена.
//! Кто вызывает: mod.rs (контракт withdraw_cost).
//! От чего зависит: check/state.rs, exec/native.rs, exec/txs.rs (Env: цена газа), wallet/transfer.rs (сеть), num.rs.
//! Настройки: venue.bsc_network [bsc_dex]; через вызываемые — send.gas_price_markup_pct, dex.native_*.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::state::Checks;
use super::super::exec::native;
use super::super::exec::txs::Env;
use super::super::num;
use super::balances::BNB;
use super::transfer::same_network;

/// Стоимость перевода: газ (пробный перевод × цена газа) и налог на перевод (% проверки монеты).
/// Ошибки: CoinNotChecked / CoinCheckFailed, сеть, узел (цена газа).
pub async fn cost(
    env: &Env,
    checks: &Checks,
    asset: &Asset,
    network: &str,
    amount: Decimal,
) -> GatewayResult<Vec<Cost>> {
    let ctx = &env.ctx;
    same_network(ctx, network)?;
    // Решение 211: налоги и газ перевода — по адресу контракта монеты.
    let token = asset.bsc_contract.ok_or_else(|| {
        ctx.err(
            ErrorCode::CoinNotChecked,
            format!("{}: нет адреса контракта — налоги не найти", asset.ticker),
        )
    })?;
    let pct = match checks.coin(&token).map(|c| c.taxes) {
        Some(CheckState::Known(t)) => t.transfer_pct,
        Some(CheckState::Failed { reason }) => {
            return Err(ctx.err(ErrorCode::CoinCheckFailed, reason));
        }
        _ => {
            return Err(ctx.err(
                ErrorCode::CoinNotChecked,
                format!("{}: налоги не проверены", asset.ticker),
            ));
        }
    };
    let native = native::is_native(ctx, asset);
    let gas = if native {
        checks.native_transfer_gas(&token)
    } else {
        checks.transfer_gas(&token)
    };
    let gas = gas.ok_or_else(|| {
        ctx.err(
            ErrorCode::CoinNotChecked,
            format!("{}: газ перевода не измерен", asset.ticker),
        )
    })?;
    let price = env.gas.wei(ctx, &env.heads).await?;
    let wei = price.saturating_mul(U256::from(gas));
    let bnb = num::to_decimal(wei, num::BNB_DECIMALS).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Газ перевода {wei} wei не переводится в BNB"),
        )
    })?;
    let mut out = vec![Cost::new(CostKind::TransferGas, bnb, BNB, false)];
    if !native {
        out.push(Cost::new(
            CostKind::TransferTax,
            amount * pct / Decimal::ONE_HUNDRED,
            &asset.ticker,
            false,
        ));
    }
    Ok(out)
}
