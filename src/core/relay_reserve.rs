//! Файл: core/relay_reserve.rs
//! Что делает: решение 417 (Тигран 19.11.2026; живые сделки 19.11 THE −15.61, SNDKB −7.56, SPCXB, SINGULARRY
//! DEX → MEXC −7.25: монета пришла на площадку продажи, продажа там в минус, «Перегон» увёз её на цель, пока монета
//! ехала 38–97 с, цена на цели упала — итог хуже оценки). Правило Тиграна: «если продать на месте выгодно — итог
//! сделки ≥ 0 — другие площадки даже не искать, продаём здесь; сделка минусовая — по расчётам понимаем, выгодно
//! перегонять или нет».
//! 1) here_first — итог сделки при продаже здесь ≥ 0 (sell.rs profitable, решение 47) — продажа здесь, цели не ищутся.
//! 2) Оценка цели перегона — с тем же запасом на движение цены за перевод, что у плана входа (reserve.rs amount):
//!    min(выручка, max(выручка · (1 − e^(−k·σ·√T)), доля закрытия · (средняя цели − средняя здесь) / средняя цели ·
//!    выручка)) — ровно reserve.rs amount (строки 125-142); доля действует, только когда средняя здесь > 0 и
//!    средняя цели выше неё.
//!    Параметры — как у плана входа направления «отсюда → на цель» (dirctx.rs build, момент Entry): σ — sigma_plan.rs
//!    entry_shared (ряд продажи на цели с напарником «отсюда», проверка profit.sigma_min_changes, граница
//!    profit.sigma_floor_per_s [цель]); T — transit.rs seconds (пара «отсюда → цель»); k — profit.price_move_k
//!    (монета → площадка + тип направления → вся система, venue_dir.rs); доля — profit.closure_share (площадка + тип
//!    направления, иначе цель). Роль «цены покупки на A» плана у перегона — средняя котировки продажи здесь (монета уже
//!    здесь, разница цен — между здесь и целью); котировки здесь нет — доля не действует (как reserve.rs при цене ≤ 0).
//!    Не вычисляется (σ не измерена, не задана настройка, T не вычисляется) — ошибка с причиной: цель не оценена, как
//!    план входа без этих данных (сделки нет).
//! 3) better — перегон только если оценка цели С ЗАПАСОМ больше выручки здесь (выручка здесь — чистыми, после затрат
//!    продажи здесь); порога-настройки нет.
//!
//! Кто вызывает: core/relay.rs, core/relay_here.rs (here_first).
//! От чего зависит: sigma_plan.rs, transit.rs, venue_dir.rs, reserve.rs (closure_share, keep_of, amount), sell.rs
//! (profitable), deal.rs, notify/human.rs (числа в тексте).
//! Настройки (новых нет): profit.price_move_k, profit.closure_share, profit.sigma_window_min, profit.sigma_min_changes,
//! profit.sigma_floor_per_s, profit.transit_mode, profit.transit_manual_s, profit.transit_credit_s,
//! profit.transit_auto_after — те же, что у плана входа.

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{Asset, Direction, VenueId};
use crate::notify::human as h;
use crate::settings::read::At;

use super::deal::Deal;
use super::engine::Core;

/// Параметры запаса перегона (как у плана входа направления «отсюда → цель»).
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// profit.price_move_k.
    pub k: Decimal,
    /// σ продажи на цели за секунду (доля).
    pub sigma: Decimal,
    /// Время перевода «отсюда → цель», с.
    pub transit_s: Decimal,
    /// profit.closure_share; None — не задана.
    pub share: Option<Decimal>,
    /// k·σ·√T.
    pub c: Decimal,
    /// e^(−k·σ·√T) — reserve.rs keep_of.
    pub keep: Decimal,
}

/// Что делает: параметры запаса перегона с площадки `from` на `to` на момент `now` — теми же функциями, что план входа
/// (dirctx.rs build, Entry). Принимает: ядро, откуда, куда, монету, сейчас (тесты — время записи).
/// Возвращает: Params. Ошибки: текст — σ не измерена, не задана настройка, неизвестный режим T, √T не вычисляется.
pub fn params_at(
    core: &Core,
    from: &VenueId,
    to: &VenueId,
    a: &Asset,
    now: DateTime<Utc>,
) -> Result<Params, String> {
    let dir = Direction {
        asset: a.clone(),
        buy_on: from.clone(),
        sell_on: to.clone(),
    };
    let s = &core.inner.deps.settings;
    let sigma = super::sigma_plan::entry_shared(&core.inner.vol, s, &dir, now)?;
    let transit_s = super::transit::seconds(core, &dir)?;
    let coin = super::coin_key::scope_id(a);
    let vd = super::venue_dir::of(core, &dir);
    let coin_at = At {
        venue: None,
        coin: Some(&coin),
    };
    let k = s
        .decimal_coin_dir("profit.price_move_k", vd, coin_at)
        .map_err(|e| e.message)?;
    let share = super::reserve::closure_share(s, &dir, vd)?;
    // Тот же текст, что у расчёта сделки (costs.rs), когда keep нет.
    let no_t = || "время в пути не вычисляется".to_string();
    let root = transit_s.sqrt().ok_or_else(no_t)?;
    let keep = super::reserve::keep_of(k, sigma, transit_s).ok_or_else(no_t)?;
    Ok(Params {
        k,
        sigma,
        transit_s,
        share,
        c: k * sigma * root,
        keep,
    })
}

/// Оценка цели перегона с запасом.
#[derive(Debug, Clone, PartialEq)]
pub struct Est {
    /// Чистая выручка перегона без запаса (relay_value.rs).
    pub raw: Decimal,
    /// Запас r (reserve.rs amount).
    pub reserve: Decimal,
    /// Базовая часть: выручка · (1 − e^(−k·σ·√T)).
    pub base: Decimal,
    /// Часть доли закрытия; None — доля не задана или не действует (цена цели не выше цены здесь, цены здесь нет).
    pub closure: Option<Decimal>,
    pub p: Params,
}

impl Est {
    /// Оценка с запасом: без запаса минус запас. Ошибок нет.
    pub fn net(&self) -> Decimal {
        self.raw - self.reserve
    }
}

/// Что делает: оценка цели с запасом (чистая функция). Принимает: выручку без запаса, выручку продажи на цели до затрат
/// (приход × средняя цели; приход-пыль — 0), среднюю продажи на цели, среднюю продажи здесь (None — котировки нет),
/// параметры. Возвращает: Est. Ошибок нет.
pub fn estimate(
    raw: Decimal,
    proceeds: Decimal,
    target_avg: Decimal,
    here_avg: Option<Decimal>,
    p: Params,
) -> Est {
    let here = here_avg.unwrap_or(Decimal::ZERO);
    let reserve = super::reserve::amount(p.keep, proceeds, p.share, here, target_avg);
    let base = proceeds * (Decimal::ONE - p.keep);
    // Только для текста: та же часть доли, что внутри reserve.rs amount (там же условие действия).
    let closure = p
        .share
        .filter(|_| here > Decimal::ZERO && target_avg > here)
        .map(|sh| sh * (target_avg - here) / target_avg * proceeds);
    Est {
        raw,
        reserve,
        base,
        closure,
        p,
    }
}

/// Правило 1 решения 417: продажа здесь выгодна (итог сделки ≥ 0, sell.rs profitable) — цели не ищутся.
/// Принимает: сделку, выручку здесь (None — котировки нет). Ошибок нет.
pub fn here_first(deal: &Deal, here: Option<Decimal>) -> bool {
    here.is_some_and(|v| super::sell::profitable(deal, v))
}

/// Правило 2 решения 417: перегон лучше продажи здесь — оценка цели с запасом больше выручки здесь (строго).
/// Принимает: выручку здесь (None — котировки нет: сравнивать не с чем, как раньше), оценку цели. Ошибок нет.
pub fn better(here: Option<Decimal>, e: &Est) -> bool {
    here.is_none_or(|v| e.net() > v)
}

/// Текст расчёта для хронологии и журнала: «ожидается v (без запаса w, запас r: k·σ·√T=…, доля закрытия=…)».
/// Принимает: оценку. Ошибок нет.
pub fn text(e: &Est) -> String {
    let share = match (e.p.share, e.closure) {
        (None, _) => "не задана".to_string(),
        (Some(sh), Some(c)) => format!("{} → {}", h::qty(sh), h::usd(c)),
        (Some(sh), None) => format!(
            "{} (не действует: цены здесь нет или цена цели не выше)",
            h::qty(sh)
        ),
    };
    format!(
        "ожидается {} (без запаса {}, запас {}: k·σ·√T={} (k={}, σ={}/с, T={} с) → {}, доля закрытия={share})",
        h::signed_usd(e.net()),
        h::signed_usd(e.raw),
        h::usd(e.reserve),
        h::qty(e.p.c),
        h::qty(e.p.k),
        h::qty(e.p.sigma),
        h::qty(e.p.transit_s),
        h::usd(e.base),
    )
}
