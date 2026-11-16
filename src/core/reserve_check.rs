//! Файл: core/reserve_check.rs
//! Что делает: решение 358 — режим сравнения оценки σ запаса на движение цены ТОЛЬКО В ЖУРНАЛЕ. При log.sigma_horizon
//! = да план входа (dirctx.rs build) несёт рядом со σ плана: σ ряда продажи на B (vol.rs measure, как сейчас), σ на
//! горизонте T того же ряда и окна (vol_horizon.rs), отношение дисперсий σ_T²/σ², автокорреляцию шагов ρ₁, число
//! отрезков и шагов; расчёт (costs.rs) добавляет запас по σ_T (та же формула reserve.rs amount, то же k, √T, доля
//! закрытия, граница profit.sigma_floor_per_s [B]) и прибыль с ним. Решение о сделке — по старой σ, как раньше:
//! эти поля только пишутся в Calc (журнал отказа кандидата core.candidate_rejected, строка «Кандидатов», план сделки).
//! Зачем: разбор docs/notes/t161-price-risk-2026-11-16.md — доказать завышение σ на шуме цены нельзя без рядов цены,
//! их нет в записях; с этим полем факт виден на живом боте.
//! Не задана или «нет» — поля нет (None, в JSON не пишется), счёта нет.
//! Обёртки entry и priced_box — обычные (не async) функции, отдают Option<Box<…>>: в будущих пути расчёта и
//! предпроверки (стек, CI t161) не остаётся ни SigmaCheck, ни промежуточных значений — только указатель.
//! Кто вызывает: core/dirctx.rs (entry — план входа), core/costs.rs (priced_box); of и priced — тесты и обёртки.
//! От чего зависит: core/vol.rs, core/vol_horizon.rs, core/reserve.rs, core/convert.rs, settings.
//! Настройки: log.sigma_horizon, profit.sigma_window_min, profit.sigma_floor_per_s [B] (через reserve.rs).

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, MathematicalOps};
use serde::Serialize;

use crate::core::types::{Cost, CostKind, Direction, Quote, Side};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::convert::{self, Rates};
use super::costs_types::CostLine;
use super::dirctx::DirCtx;
use super::engine::{Core, lk};
use super::vol::Volatility;
use super::vol_horizon::Horizon;

/// Сравнение σ запаса (только журнал, решение 358).
#[derive(Debug, Clone, Serialize)]
pub struct SigmaCheck {
    /// Горизонт T, секунды (время в пути плана).
    pub horizon_s: Decimal,
    /// σ плана — по ней считан запас и принято решение (с границей profit.sigma_floor_per_s).
    pub sigma_plan: Decimal,
    /// σ ряда без границы (vol.rs measure); None — не измерена.
    pub sigma_series: Option<Decimal>,
    /// σ на горизонте T и признаки ряда (vol_horizon.rs); None — ряда нет.
    pub horizon: Option<Horizon>,
    /// σ_T с той же границей profit.sigma_floor_per_s [B], что у σ плана.
    pub sigma_t_plan: Option<Decimal>,
    /// σ_T² / σ ряда² — во сколько раз «σ ряда, растянутая на T» больше (< 1) или меньше (> 1) движения за T.
    pub variance_ratio: Option<Decimal>,
    /// Запас по σ_T плана, USDT, и прибыль расчёта с ним вместо прежнего запаса (заполняет costs.rs).
    pub reserve_t_usdt: Option<Decimal>,
    pub profit_t_usd: Option<Decimal>,
    /// Почему части нет (окно не задано, σ не измерена, граница в неверном виде).
    pub note: Option<String>,
}

/// Что делает: сравнение σ для плана входа (см. шапку). Принимает: ряды σ, хранилище, направление, σ плана, T плана,
/// сейчас. Возвращает: None — log.sigma_horizon не «да». Ошибок нет: причины — в поле note.
pub fn of(
    vol: &mut Volatility,
    s: &Store,
    d: &Direction,
    sigma_plan: Decimal,
    horizon_s: Decimal,
    now: DateTime<Utc>,
) -> Option<SigmaCheck> {
    if !s.flag("log.sigma_horizon", At::default()).unwrap_or(false) {
        return None;
    }
    let mut c = SigmaCheck {
        horizon_s,
        sigma_plan,
        sigma_series: None,
        horizon: None,
        sigma_t_plan: None,
        variance_ratio: None,
        reserve_t_usdt: None,
        profit_t_usd: None,
        note: None,
    };
    let window = match s.integer("profit.sigma_window_min", At::default()) {
        Ok(w) => w,
        Err(e) => {
            c.note = Some(e.message);
            return Some(c);
        }
    };
    let (b, a) = (&d.sell_on, &d.buy_on);
    match vol.measure(b, &d.asset, Side::Sell, a, window, now) {
        Ok((sigma, _)) => c.sigma_series = Some(sigma),
        Err(e) => c.note = Some(e),
    }
    c.horizon = vol.horizon(b, &d.asset, Side::Sell, a, window, now, horizon_s);
    let floor =
        match super::reserve::opt(s.decimal("profit.sigma_floor_per_s", super::reserve::at_b(d))) {
            Ok(f) => f,
            Err(e) => {
                c.note = Some(e);
                None
            }
        };
    let sigma_t = c.horizon.as_ref().and_then(|h| h.sigma_t);
    c.sigma_t_plan = sigma_t.map(|st| floor.map_or(st, |f| st.max(f)));
    if let (Some(st), Some(ss)) = (sigma_t, c.sigma_series)
        && !ss.is_zero()
    {
        c.variance_ratio = Some((st / ss).powi(2));
    }
    Some(c)
}

/// Что делает: запас по σ_T плана и прибыль с ним (только поля сравнения). Принимает: сравнение, контекст направления
/// (k, доля закрытия, котируемая монета A), √T, выручку, средние цены покупки и продажи, прежний запас в USDT, прибыль
/// расчёта, курсы затрат. Возвращает: копию с reserve_t_usdt и profit_t_usd (None — нет σ_T или курса). Ошибок нет.
#[allow(clippy::too_many_arguments)]
pub fn priced(
    c: &SigmaCheck,
    cx: &DirCtx,
    sqrt_t: Decimal,
    proceeds: Decimal,
    buy_avg: Decimal,
    sell_avg: Decimal,
    reserve_usdt: Decimal,
    profit: Decimal,
    rates: &Rates,
) -> SigmaCheck {
    let mut out = c.clone();
    let Some(st) = c.sigma_t_plan else {
        return out;
    };
    let amount = super::reserve::amount(
        cx.k,
        st,
        sqrt_t,
        proceeds,
        cx.closure_share,
        buy_avg,
        sell_avg,
    );
    let cost = Cost::new(
        CostKind::PriceMoveReserve,
        amount,
        cx.quote[0].clone(),
        false,
    );
    if let Ok(u) = convert::to_usdt(&cost, rates) {
        out.reserve_t_usdt = Some(u);
        out.profit_t_usd = Some(profit + reserve_usdt - u);
    }
    out
}

/// Что делает: сравнение σ плана входа для DirCtx (of по рядам ядра, сейчас) — в куче. Принимает: ядро, хранилище,
/// направление, σ и T плана. Возвращает: None — log.sigma_horizon не «да». Ошибок нет.
pub fn entry(
    core: &Core,
    s: &Store,
    d: &Direction,
    sigma_plan: Decimal,
    horizon_s: Decimal,
) -> Option<Box<SigmaCheck>> {
    let mut vol = lk(&core.inner.vol);
    of(&mut vol, s, d, sigma_plan, horizon_s, Utc::now()).map(Box::new)
}

/// Что делает: запас и прибыль по σ_T для Calc (priced) — в куче. Принимает: контекст (без сравнения — None), строки
/// затрат расчёта (прежний запас в USDT — строка PriceMoveReserve), √T, выручку, котировки покупки и продажи, прибыль,
/// курсы. Возвращает: Option<Box<SigmaCheck>>. Ошибок нет.
#[allow(clippy::too_many_arguments)]
pub fn priced_box(
    cx: &DirCtx,
    costs: &[CostLine],
    sqrt_t: Decimal,
    proceeds: Decimal,
    qa: &Quote,
    qb: &Quote,
    profit: Decimal,
    rates: &Rates,
) -> Option<Box<SigmaCheck>> {
    let c = cx.sigma_check.as_deref()?;
    let r = costs
        .iter()
        .find(|l| l.kind == CostKind::PriceMoveReserve)
        .and_then(|l| l.usdt)
        .unwrap_or_default();
    let (buy, sell) = (qa.avg_price, qb.avg_price);
    Some(Box::new(priced(
        c, cx, sqrt_t, proceeds, buy, sell, r, profit, rates,
    )))
}
