//! Файл: venues/bsc_dex/exec/tip_pick.rs
//! Что делает: решение 437 (3) — чаевые сделки DEX ↔ DEX в режиме send.tip_mode = profit_share (формула —
//! /home/claude/src-ext/reports/arb-tips.md, раздел 3; утверждена Тиграном 20.11.2026):
//!   G   — газ L2 транзакции (чаевые платятся только с него: nitro arbos/tx_processor.go:627-645, exec/cycle_gas.rs);
//!   P0  — чистая прибыль цикла без чаевых (все прочие затраты вычтены), в нативной монете сети;
//!   Pmin — порог dex_dex.min_profit_usd, в нативной монете;
//!   предел по прибыли = (P0 − Pmin) / G — после чаевых прибыль не ниже порога;
//!   чаевые = min(max(s/100 × P0 / G, нижний предел), верхний предел, предел по прибыли);
//!   предел по прибыли ниже нижнего предела — сделки нет.
//! Все суммы — Decimal в нативной монете; чаевые за газ — в wei, вниз до целого (прибыль не уходит ниже порога).
//! Кто вызывает: exec/tip_cycle.rs.
//! От чего зависит: send/tip_mode.rs (ShareCfg), num.rs.
//! Настройки: нет своих (пределы — ShareCfg, читает вызывающий до отправки).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use super::super::num;
use super::super::send::tip_mode::ShareCfg;

/// Decimals нативной монеты EVM-сети (wei = 10⁻¹⁸ монеты — Ethereum Yellow Paper, «Ether»; так же num::BNB_DECIMALS).
const NATIVE_DECIMALS: u8 = num::BNB_DECIMALS;

/// Чаевые за газ. Принимает: пределы и шаги, долю s (%), (P0, Pmin) в нативной монете, газ L2.
/// Возвращает: wei за газ. Ошибки (текст причины «сделки нет»): газ L2 = 0; предел по прибыли ниже нижнего предела
/// (прибыль без чаевых не выше порога на величину нижних чаевых); число не помещается.
pub fn pick(
    c: &ShareCfg,
    s: Decimal,
    (p0, pmin): (Decimal, Decimal),
    g: u64,
) -> Result<u128, String> {
    if g == 0 {
        return Err("газ L2 транзакции — 0, чаевые за газ не посчитать".into());
    }
    let gas = Decimal::from(g);
    let room = (p0 - pmin) / gas;
    let floor = wei_dec(c.floor)?;
    if room < floor {
        return Err(format!(
            "предел чаевых по прибыли {} wei/газ ниже нижнего предела send.tip_floor_gwei \
             ({} wei/газ): прибыль без чаевых {p0} при пороге {pmin} (нативная монета), \
             газ L2 {g} — сделки нет",
            to_wei(room.max(Decimal::ZERO))?,
            c.floor
        ));
    }
    let by_share = s / Decimal::ONE_HUNDRED * p0 / gas;
    let tip = by_share.max(floor).min(wei_dec(c.cap)?).min(room);
    to_wei(tip)
}

/// wei → нативная монета (Decimal). Ошибки: текст — не помещается.
fn wei_dec(w: u128) -> Result<Decimal, String> {
    num::to_decimal(U256::from(w), NATIVE_DECIMALS).ok_or_else(|| format!("{w} wei не в Decimal"))
}

/// Нативная монета (Decimal) → wei, вниз до целого. Ошибки: текст — отрицательное или не помещается в u128.
fn to_wei(v: Decimal) -> Result<u128, String> {
    num::to_units(v, NATIVE_DECIMALS)
        .and_then(|u| u128::try_from(u).ok())
        .ok_or_else(|| format!("чаевые {v} не переводятся в wei"))
}
