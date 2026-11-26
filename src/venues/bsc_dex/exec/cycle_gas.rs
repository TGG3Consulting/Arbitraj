//! Файл: venues/bsc_dex/exec/cycle_gas.rs
//! Что делает: решение 437 (2) — стоимость газа транзакции цикла DEX ↔ DEX для проверки прибыли по симуляции перед
//! отправкой (exec/cycle_steps.rs net_check) по модели газа площадки (send/gas_mode.rs):
//! - legacy (BSC) — как было: газ симуляции × цена газа плана (eth_gasPrice + надбавка, не ниже минимума каналов);
//! - eip1559_l1 (Arbitrum) — чаевые только с газа L2: газ_L2 × (baseFee + чаевые) + газ_L1 × baseFee. Факт кода Nitro
//!   (клон OffchainLabs/nitro ccc5c828): arbos/tx_processor.go:627-645 — при сборе чаевых газ части L1 считается по
//!   полной цене (база + чаевые), «This avoids overcharging the user for the tip on the poster's data gas»: плата за
//!   данные L1 в wei от чаевых не растёт (/home/claude/src-ext/reports/arb-tips.md, Ф8). Газ L1 — gasEstimateForL1
//!   NodeInterface.gasEstimateComponents той же транзакции (node/arb_gas.rs; адрес — dex.node_interface): у узла
//!   gasForL1 = плата L1 / baseFee, поэтому газ_L1 × baseFee = плата за данные L1. Газ L2 — газ симуляции
//!   eth_simulateV1 (как и раньше в проверке); входит ли в него часть L1 — по исходникам не установлено (docs/TASKS.md,
//!   «Решение 437», вопрос 3): если входит — часть L1 посчитана дважды по базе (верхняя оценка, затраты не занижаются).
//!   Чаевые и цена — те же, с которыми уйдёт транзакция (quote/gas.rs fees + минимум каналов, send/tx_price.rs floored;
//!   в режиме profit_share — выбранные exec/tip_pick.rs, with_tip).
//!
//! Кто вызывает: exec/{cycle_exec, cycle_dry, cycle_bundle_trade}.rs, exec/cycle_steps.rs (net_check).
//! От чего зависит: quote/gas.rs, send/{gas_mode, tx_price, mod}.rs, node/arb_gas.rs, exec/cycle_steps.rs (wei).
//! Настройки: dex.gas_mode, dex.node_interface; через вызываемые — send.priority_fee_gwei, send.max_fee_margin_pct,
//! channel.* [площадка DEX].

use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::node::arb_gas;
use super::super::quote::Quoting;
use super::super::send::gas_mode::{self, GasMode};
use super::super::send::{self, tx_price};
use super::cycle_steps;

/// Газ транзакции цикла и его цена.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bill {
    /// legacy: единицы газа × одна цена (wei).
    Flat { units: u64, wei: U256 },
    /// Type-2 Ethereum/Base: базовая цена, чаевые, разовая комиссия публикации и оператора Base.
    Type2 { units: u64, base: U256, tip: u128, extra: U256 },
    /// eip1559_l1: газ L2 и газ L1 (единицы), базовая цена блока и чаевые за газ (wei).
    Split {
        l2: u64,
        l1: u64,
        base: U256,
        tip: u128,
    },
}

impl Bill {
    /// Стоимость газа, wei: Flat — units × wei; Split — l2 × (base + tip) + l1 × base. Ошибок нет (насыщение).
    pub fn wei(&self) -> U256 {
        match *self {
            Self::Type2 { units, base, tip, extra } => base.saturating_add(U256::from(tip)).saturating_mul(U256::from(units)).saturating_add(extra),
            Self::Flat { units, wei } => wei.saturating_mul(U256::from(units)),
            Self::Split { l2, l1, base, tip } => {
                let l2_price = base.saturating_add(U256::from(tip));
                let l2_cost = l2_price.saturating_mul(U256::from(l2));
                l2_cost.saturating_add(base.saturating_mul(U256::from(l1)))
            }
        }
    }

    /// Цена единицы газа исполнения (для газа перевода чаевых канала, send/tip_gas.rs): Flat — wei; Split — база +
    /// чаевые. Ошибок нет.
    pub fn price(&self) -> U256 {
        match *self {
            Self::Flat { wei, .. } => wei,
            Self::Type2 { base, tip, .. } | Self::Split { base, tip, .. } => base.saturating_add(U256::from(tip)),
        }
    }

    /// Все единицы газа (для текста). Ошибок нет.
    pub fn units(&self) -> u64 {
        match *self {
            Self::Type2 { units, .. } | Self::Flat { units, .. } => units,
            Self::Split { l2, l1, .. } => l2.saturating_add(l1),
        }
    }

    /// Та же цена с другим газом симуляции (у Split — газ L2). Ошибок нет.
    pub fn with_units(self, u: u64) -> Self {
        match self {
            Self::Type2 { base, tip, extra, .. } => Self::Type2 { units: u, base, tip, extra },
            Self::Flat { wei, .. } => Self::Flat { units: u, wei },
            Self::Split { l1, base, tip, .. } => Self::Split {
                l2: u,
                l1,
                base,
                tip,
            },
        }
    }

    /// Та же цена с другими чаевыми (решение 437 (3), profit_share); у Flat — без изменений. Ошибок нет.
    pub fn with_tip(self, t: u128) -> Self {
        match self {
            Self::Type2 { units, base, extra, .. } => Self::Type2 { units, base, tip: t, extra },
            Self::Split { l2, l1, base, .. } => Self::Split {
                l2,
                l1,
                base,
                tip: t,
            },
            flat => flat,
        }
    }
}

/// Цена газа по модели площадки, газ симуляции — 0 (ставит with_units, когда симуляция готова: этот вызов идёт
/// одновременно с ней). Принимает: котировку, отправителя, адрес и данные транзакции (для gasEstimateComponents у
/// eip1559_l1). Возвращает: Bill.
/// Ошибки: NotConfigured (dex.gas_mode, dex.node_interface, чаевые, запас, каналы), StaleData, BadResponse, узел.
pub async fn priced(
    q: &Quoting,
    from: Address,
    (to, data): (Address, Bytes),
) -> GatewayResult<Bill> {
    let ctx = &q.ctx;
    let mode = gas_mode::read(ctx)?;
    if mode == GasMode::Legacy {
        let wei = cycle_steps::wei(q).await?;
        return Ok(Bill::Flat { units: 0, wei });
    }
    let floor = send::planned_floor(ctx)?;
    if matches!(mode, GasMode::Ethereum | GasMode::OpStack) {
        let (f, base) = q.gas.fees(ctx, &q.heads).await?;
        let f = tx_price::floored(f, floor, base)
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "цена газа не помещается в u128"))?;
        return Ok(Bill::Type2 { units: 0, base, tip: f.tip, extra: U256::ZERO });
    }
    let ((f, base), c) = tokio::try_join!(
        q.gas.fees(ctx, &q.heads),
        arb_gas::components(ctx, from, to, false, data)
    )?;
    let f = tx_price::floored(f, floor, base)
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "цены газа не помещаются в u128"))?;
    Ok(Bill::Split {
        l2: 0,
        l1: c.gas_for_l1,
        base,
        tip: f.tip,
    })
}

/// Добавляет измеренный газ исполнения и OP fee каждой транзакции. У DEX цикла одна транзакция обмена.
pub async fn simulated(q: &Quoting, bill: Bill, swap: u64, approvals: usize, approve_gas: u64) -> GatewayResult<Bill> {
    let bill = bill.with_units(swap.saturating_add(approve_gas));
    if let Bill::Type2 { units, base, tip, .. } = bill {
        let extra = super::super::node::op_fees::planned(&q.ctx, &q.gas, swap).await?;
        let approve = if approvals == 0 { U256::ZERO } else {
            // Общий газ разрешений — верхняя оценка для каждого отдельного вызова.
            super::super::node::op_fees::planned(&q.ctx, &q.gas, approve_gas).await?.saturating_mul(U256::from(approvals))
        };
        return Ok(Bill::Type2 { units, base, tip, extra: extra.saturating_add(approve) });
    }
    Ok(bill)
}

/// Газ без разбиения (бандл — только legacy, exec/bundle_send.rs): единицы × цена плана. Принимает: котировку,
/// единицы. Ошибки: как cycle_steps::wei.
pub async fn flat(q: &Quoting, units: u64) -> GatewayResult<Bill> {
    let wei = cycle_steps::wei(q).await?;
    Ok(Bill::Flat { units, wei })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Записи своего узла Arbitrum (05_*.json): газ 0x5765, из них L1 — 0x3d3, baseFee 0x1346150. При чаевых 0
    /// стоимость Split = весь газ × base (как без разбиения); при чаевых t — прибавка только t × газ_L2.
    #[test]
    fn tip_only_on_l2_recorded_components() {
        let rec: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/arbitrum_node_2026-11-19/05_nodeinterface_gasEstimateComponents.json"
        ))
        .unwrap();
        let hex = rec["response"]["result"].as_str().unwrap();
        let out = alloy_primitives::hex::decode(hex).unwrap();
        let c = arb_gas::decode(&out).unwrap();
        let split = Bill::Split {
            l2: c.l2_gas(),
            l1: c.gas_for_l1,
            base: c.base_fee,
            tip: 0,
        };
        let flat = Bill::Flat {
            units: c.gas_estimate,
            wei: c.base_fee,
        };
        assert_eq!(split.wei(), flat.wei());
        let t = 1_000_000u128;
        let extra = U256::from(t) * U256::from(c.l2_gas());
        assert_eq!(split.with_tip(t).wei(), flat.wei() + extra);
    }
}
