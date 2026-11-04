//! Файл: venues/bsc_dex/quote/verify_decide.rs
//! Что делает: чистая часть предторговой верификации (quote/verify.rs, решение 83): пересчёт выбранного маршрута на
//! СВЕЖИХ состояниях пулов нашей математикой и вердикт по дрейфу цены. Без обращений к узлу — тестируется на записях.
//! Восстанавливает ноги каждой доли из её хопов (порядок исполнения стороны), считает route_calc::eval и собирает
//! котировку costs::quote. Вердикт (решение 121, часть B4): свежая средняя цена ХУЖЕ границы сделки ядра
//! (req.worst_price: покупка — дороже, продажа — дешевле) → Reject; лучше плана — не повод отменять. Дрейф средней
//! цены от плана в любую сторону больше verify.max_price_drift_pct — тоже Reject, но это только грубый предохранитель
//! от битых данных (советуемое 10–20 %). Иначе Ok с amountOutMin по каждой ноге (решение 84 = ожидаемый выход ×
//! (1 − exec.dex_slippage_pct)). Порог прибыли — ядро.
//! Кто вызывает: quote/verify.rs (run); тесты.
//! От чего зависит: quote/verify.rs (Verdict), quote/route_calc.rs, quote/graph_legs.rs, quote/select.rs,
//! quote/costs.rs, quote/steps.rs, quote/via.rs, pools/registry.rs.
//! Настройки: своих не читает — грубый предохранитель (verify.max_price_drift_pct), допуск проскальзывания
//! (exec.dex_slippage_pct) и границу сделки (req.worst_price) передаёт вызывающий в Bounds (quote/verify.rs run читает
//! настройки через ?: не задано — верификация не работает, как в боевом пути exec/order.rs, exec/chain.rs).

use std::collections::HashMap;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{PoolRef, Quote, RouteHop, RouteMode, Side};

use super::super::pools::registry::Entry;
use super::route_calc::{self, Leg};
use super::steps::Names;
use super::verify::Verdict;
use super::via::Via;
use super::{costs, graph_legs, pool_tax, select};

/// Пределы вердикта верификации (решение 121, часть B4).
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// Грубый предохранитель от битых данных: дрейф средней цены от плана в любую сторону, % (verify.max_price_drift_pct).
    pub max_drift: Decimal,
    /// Допуск проскальзывания для amountOutMin, % (exec.dex_slippage_pct, решение 84).
    pub slip_pct: Decimal,
    /// Граница сделки из плана ядра (req.worst_price): покупка — не дороже, продажа — не дешевле.
    pub worst: Decimal,
}

/// Пересчёт маршрута на свежих состояниях и вердикт. Принимает: расчёт путей, тикеры, план, свежие записи пулов
/// маршрута, цену газа, блок перечтения, пределы (Bounds — вызывающий читает настройки через ?: не заданы →
/// верификация не работает). Возвращает: вердикт. Ошибок нет — неудача пересчёта это Reject.
pub fn decide(
    v: &Via<'_>,
    names: &Names<'_>,
    plan: &Quote,
    fresh: &HashMap<PoolRef, Entry>,
    gas: U256,
    block: u64,
    b: Bounds,
) -> Verdict {
    let ctx = &v.q.ctx;
    let Some(route) = plan.route.as_ref() else {
        return reject("нет маршрута для верификации".into());
    };
    let mut paths = Vec::with_capacity(route.legs.len());
    for leg in &route.legs {
        let legs = match rebuild(v, &leg.hops, fresh) {
            Ok(l) => l,
            Err(w) => return reject(w),
        };
        // Количество доли: покупка — её монеты (expected_out), продажа — доля входа × всего монет.
        let qty = match v.side {
            Side::Buy => leg.expected_out,
            Side::Sell => leg.share * plan.qty,
        };
        // Решение 146: налог монеты — пула шага монеты этой доли (quote/pool_tax.rs).
        let tax_keep = pool_tax::coin_keep(&legs, v.side, v.taxes);
        match route_calc::eval(ctx, &legs, v.side, qty, tax_keep) {
            Ok(p) => paths.push(p),
            Err(e) => return reject(e.message),
        }
    }
    let fresh_quote = match costs::quote(
        ctx,
        &plan.asset,
        v.side,
        plan.qty,
        &paths,
        RouteMode::Graph,
        v.taxes,
        gas,
        names,
    ) {
        Ok(q) => q,
        Err(e) => return reject(e.message),
    };
    if plan.avg_price.is_zero() {
        return reject("цена плана нулевая".into());
    }
    if b.worst <= Decimal::ZERO {
        return reject("граница сделки (худшая цена) не положительная".into());
    }
    let fresh_avg = fresh_quote.avg_price;
    let drift = ((fresh_avg - plan.avg_price).abs() / plan.avg_price) * Decimal::ONE_HUNDRED;
    let r = |x: Decimal| x.round_dp(8).normalize();
    // Грубый предохранитель (решение 121, B4): скачок в ЛЮБУЮ сторону больше допуска — скорее битые данные, чем рынок.
    if drift > b.max_drift {
        return Verdict::Reject {
            reason: format!(
                "цена уехала на {}% — больше грубого предохранителя verify.max_price_drift_pct = {} (данные под \
                 сомнением)",
                drift.round_dp(4).normalize(),
                b.max_drift
            ),
            drift: Some(drift),
        };
    }
    // Решение по выгоде (B4): отказ только если свежая средняя цена ХУЖЕ границы сделки ядра; лучше — сделка идёт.
    let worse = match v.side {
        Side::Buy => fresh_avg > b.worst,
        Side::Sell => fresh_avg < b.worst,
    };
    if worse {
        let how = if v.side == Side::Buy {
            "дороже"
        } else {
            "дешевле"
        };
        return Verdict::Reject {
            reason: format!(
                "свежая средняя цена {} {how} границы сделки {} (худшая цена плана ядра)",
                r(fresh_avg),
                r(b.worst)
            ),
            drift: Some(drift),
        };
    }
    // amountOutMin по ноге (решение 84) = ожидаемый выход × (1 − exec.dex_slippage_pct). Допуск передан вызывающим
    // через ? (не задан → до сюда не доходим), а не читается здесь с «тихим нулём» — конвенция боевого пути (order.rs).
    let keep = Decimal::ONE - b.slip_pct / Decimal::ONE_HUNDRED;
    let amount_out_min = fresh_quote
        .route
        .as_ref()
        .map(|r| r.legs.iter().map(|l| l.expected_out * keep).collect())
        .unwrap_or_default();
    Verdict::Ok {
        fresh_quote: Box::new(fresh_quote),
        amount_out_min,
        block,
    }
}

/// Вердикт-отказ без дрейфа. Ошибок нет.
fn reject(reason: String) -> Verdict {
    Verdict::Reject {
        reason,
        drift: None,
    }
}

/// Ноги одной доли из её хопов на свежих записях (порядок исполнения стороны, как в маршруте). Токен-путь — от старта
/// стороны (USDT покупки / монета продажи), другой конец пула — выход. Ошибка (текст) — пул не перечитан, не допущен
/// или нет decimals.
fn rebuild(
    v: &Via<'_>,
    hops: &[RouteHop],
    fresh: &HashMap<PoolRef, Entry>,
) -> Result<Vec<Leg>, String> {
    let mut running = if v.side == Side::Buy { v.usdt } else { v.coin };
    let mut legs = Vec::with_capacity(hops.len());
    for h in hops {
        let e = fresh.get(&h.pool).ok_or("пул маршрута не перечитан")?;
        let token_in = running;
        let token_out = if e.pool.token0 == token_in {
            e.pool.token1
        } else {
            e.pool.token0
        };
        let hs = graph_legs::hop_side(e, token_in).ok_or("ребро не соединяет токены хопа")?;
        let usable = select::usable(v.q, e, hs, v.usdt_dec)
            .map_err(|w| format!("{} {}: {w}", e.family.title, e.ticker))?;
        let dec_in =
            v.q.tokens
                .decimals_known(token_in)
                .ok_or("нет decimals токена входа")?;
        let dec_out =
            v.q.tokens
                .decimals_known(token_out)
                .ok_or("нет decimals токена выхода")?;
        legs.push(Leg {
            entry: e.clone(),
            usable,
            token_in,
            token_out,
            dec_in,
            dec_out,
        });
        running = token_out;
    }
    Ok(legs)
}
