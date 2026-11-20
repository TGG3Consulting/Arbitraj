//! Файл: venues/bsc_dex/send/tip_mode.rs
//! Что делает: решение 437 (3) — режим чаевых транзакции типа 2 у площадки с dex.gas_mode = eip1559_l1 (Arbitrum),
//! настройка send.tip_mode: `fixed` — чаевые send.priority_fee_gwei (как до решения 437); `profit_share` — у сделки
//! DEX ↔ DEX чаевые — доля прибыли цикла (exec/tip_pick.rs) в пределах send.tip_floor_gwei … send.tip_cap_gwei, доля
//! меняется по итогам (exec/tip_state.rs) в пределах send.tip_share_min_pct … send.tip_share_max_pct шагами
//! send.tip_share_step_up_pct / send.tip_share_step_down_pct после send.tip_wins_to_step_down побед подряд. У прочих
//! транзакций площадки (разрешения, переводы, ноги CEX ↔ DEX) и в плане котировки в режиме profit_share — нижний
//! предел send.tip_floor_gwei (чаевые сделки не ниже него: предел по прибыли ниже — сделки нет).
//! Значений по умолчанию нет: не задана нужная настройка — NotConfigured (котировок и отправки площадки нет,
//! «Проверить подключение», часть «Газ сети» — причина). При legacy (BSC) не читается.
//! Кто вызывает: quote/gas_base.rs (tip_wei), exec/tip_cycle.rs, connection_gas.rs.
//! От чего зависит: ctx.rs, num.rs; варианты — settings/catalog/send.rs TIP_MODES (тест сверяет).
//! Настройки: send.tip_mode, send.priority_fee_gwei (fixed), send.tip_floor_gwei, send.tip_cap_gwei,
//! send.tip_share_min_pct, send.tip_share_max_pct, send.tip_share_step_up_pct, send.tip_share_step_down_pct,
//! send.tip_wins_to_step_down (profit_share) [площадка DEX].
//!
//! Механизм — /home/claude/src-ext/reports/arb-tips.md, раздел 3 (утверждён Тиграном 20.11.2026): порядок секвенсора
//! Arbitrum One — PGA по чаевым внутри раунда (nitro execution/gethexec/pga/tx.go:43-46, pga_adapter.go:391-404).

use rust_decimal::Decimal;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::num;

/// Пределы и шаги режима profit_share (чаевые — wei за газ, доли — %).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareCfg {
    pub floor: u128,
    pub cap: u128,
    pub min_pct: Decimal,
    pub max_pct: Decimal,
    pub up_pct: Decimal,
    pub down_pct: Decimal,
    pub wins_to_down: u64,
}

/// Режим чаевых.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipMode {
    /// send.priority_fee_gwei, wei.
    Fixed(u128),
    ProfitShare(ShareCfg),
}

impl TipMode {
    /// Чаевые транзакции без своей прибыли и плана котировки: fixed — send.priority_fee_gwei; profit_share — нижний
    /// предел. Ошибок нет.
    pub fn base_tip(&self) -> u128 {
        match self {
            Self::Fixed(t) => *t,
            Self::ProfitShare(c) => c.floor,
        }
    }

    /// Текст режима (как в настройке). Ошибок нет.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fixed(_) => "fixed",
            Self::ProfitShare(_) => "profit_share",
        }
    }
}

/// Читает режим. Принимает: окружение. Возвращает: TipMode.
/// Ошибки: NotConfigured — send.tip_mode не задана или не из списка; нужная режиму настройка не задана или неверна
/// (gwei не в wei; нижний предел выше верхнего; минимум доли выше максимума или максимум больше 100 %).
pub fn read(ctx: &Ctx) -> GatewayResult<TipMode> {
    let m = ctx.text("send.tip_mode")?;
    match m.trim() {
        "fixed" => Ok(TipMode::Fixed(gwei(ctx, "send.priority_fee_gwei")?)),
        "profit_share" => Ok(TipMode::ProfitShare(share(ctx)?)),
        other => {
            let why = format!("send.tip_mode = «{other}»: нет режима (fixed или profit_share)");
            Err(ctx.not_set("send.tip_mode", why))
        }
    }
}

/// Настройки profit_share с проверкой пределов. Ошибки: как read.
fn share(ctx: &Ctx) -> GatewayResult<ShareCfg> {
    let c = ShareCfg {
        floor: gwei(ctx, "send.tip_floor_gwei")?,
        cap: gwei(ctx, "send.tip_cap_gwei")?,
        min_pct: ctx.dec("send.tip_share_min_pct")?,
        max_pct: ctx.dec("send.tip_share_max_pct")?,
        up_pct: ctx.dec("send.tip_share_step_up_pct")?,
        down_pct: ctx.dec("send.tip_share_step_down_pct")?,
        wins_to_down: ctx.int("send.tip_wins_to_step_down")?,
    };
    if c.floor > c.cap {
        let why = "send.tip_floor_gwei больше send.tip_cap_gwei — пределы чаевых перепутаны";
        return Err(ctx.not_set("send.tip_floor_gwei", why));
    }
    if c.min_pct > c.max_pct || c.max_pct > Decimal::ONE_HUNDRED {
        let why = "send.tip_share_min_pct больше send.tip_share_max_pct или максимум больше 100 %";
        return Err(ctx.not_set("send.tip_share_max_pct", why));
    }
    Ok(c)
}

/// gwei настройки → wei (вниз до целого). Ошибки: NotConfigured — не задана, отрицательная или не помещается в u128.
fn gwei(ctx: &Ctx, name: &str) -> GatewayResult<u128> {
    let g = ctx.dec(name)?;
    num::to_units(g * Decimal::from(1_000_000_000u64), 0)
        .and_then(|v| u128::try_from(v).ok())
        .ok_or_else(|| ctx.not_set(name, format!("{name} = {g} не переводится в wei")))
}

/// wei → gwei для показа. Ошибок нет (не помещается — None).
pub fn to_gwei(wei: u128) -> Option<Decimal> {
    num::to_decimal(alloy_primitives::U256::from(wei), 9)
}

#[cfg(test)]
mod tests {
    use crate::settings::catalog::send::TIP_MODES;

    /// Варианты каталога — ровно те, что разбирает read.
    #[test]
    fn catalog_and_code_agree() {
        assert_eq!(TIP_MODES, &["fixed", "profit_share"]);
    }
}
