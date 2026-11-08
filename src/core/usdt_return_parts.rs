//! Файл: core/usdt_return_parts.rs
//! Что делает: решение 227 — возврат выручки USDT закрытой сделки ПО ЧАСТЯМ: часть = площадка, где лежит выручка
//! продаж сделки (deal.sells по площадкам), сумма части = выручка продаж на этой площадке минус их комиссия в котируемой
//! монете этой площадки; каждая часть — своим переводом на площадку покупки. Живой случай 20261108104958308JCT
//! (bingx → bitget): часть продана на Bitget (≈137.10 USDT), остаток перегнан на DEX (w7) и продан там (≈342.29 USDT);
//! возврат шёл одним перегоном с bsc_dex суммой всей выручки (479.25), повтор забрал все свободные USDT w7 (387.56 —
//! вместе с оборотными деньгами другого направления), 136.96 USDT остались на Bitget, возврат помечен «вернулись».
//! Хранение (совместимо со старыми сделками): Deal.usdt_return — первая часть (площадка, где сделка закрылась, —
//! как до решения 227), Deal.usdt_return_parts — остальные (serde default: у старых сделок пусто). Часть узнаётся по
//! площадке, где лежат USDT (у каждой площадки не больше одной части).
//! Ключ части для команд экрана «Ручное управление» и Telegram: первая — id сделки (как раньше), остальные —
//! «<id>@<площадка>» (поле deal_id команд UsdtRetry / UsdtReturnedManually).
//! Кто вызывает: usdt_return_deal.rs (части при закрытии), usdt_return_state.rs (set, on_restart),
//! usdt_return_retry.rs / usdt_return_attempt.rs (часть повтора), manual_usdt.rs, manual_feed.rs, тесты.
//! От чего зависит: deal.rs, usdt_return_state.rs (UsdtReturn).
//! Настройки: нет (котируемая монета площадки и допуск usdt.return_tolerance_usd — у вызывающего).

use rust_decimal::Decimal;

use crate::core::types::VenueId;

use super::deal::Deal;
use super::usdt_return_state::UsdtReturn;

impl UsdtReturn {
    /// Площадка, где лежат USDT этой части (откуда перегон). Ошибок нет.
    pub fn venue(&self) -> &VenueId {
        match self {
            Self::Pending { from, .. } | Self::Done { from, .. } | Self::Retrying { from, .. } => {
                from
            }
            Self::NotReturned { at, .. } | Self::Manual { at, .. } => at,
        }
    }

    /// Часть закончена: вернулись или перегнано вручную. Ошибок нет.
    pub fn finished(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Manual { .. })
    }
}

/// Все части возврата сделки: первая (Deal.usdt_return), затем остальные. Ошибок нет.
pub fn all(d: &Deal) -> Vec<&UsdtReturn> {
    d.usdt_return
        .iter()
        .chain(d.usdt_return_parts.iter())
        .collect()
}

/// Возврат сделки закончен — ВСЕ части вернулись или перегнаны вручную (решение 227). Частей нет — false. Ошибок нет.
pub fn all_done(d: &Deal) -> bool {
    let v = all(d);
    !v.is_empty() && v.iter().all(|p| p.finished())
}

/// Часть по площадке, где лежат USDT. Принимает: сделку, площадку (None — первая часть). Ошибок нет.
pub fn get<'a>(d: &'a Deal, at: Option<&VenueId>) -> Option<&'a UsdtReturn> {
    match at {
        None => d.usdt_return.as_ref(),
        Some(v) => all(d).into_iter().find(|p| p.venue() == v),
    }
}

/// Ставит состояние части (по её площадке): заменяет часть с той же площадкой, иначе — первой (если её нет) или
/// новой остальной. Принимает: сделку, состояние. Ошибок нет.
pub fn put(d: &mut Deal, state: UsdtReturn) {
    let v = state.venue().clone();
    match &d.usdt_return {
        None => d.usdt_return = Some(state),
        Some(p) if *p.venue() == v => d.usdt_return = Some(state),
        Some(_) => match d.usdt_return_parts.iter_mut().find(|p| *p.venue() == v) {
            Some(x) => *x = state,
            None => d.usdt_return_parts.push(state),
        },
    }
}

/// Ключ части для команд (решение 227): первая часть — id сделки, остальные — «<id>@<площадка>». Ошибок нет.
pub fn key(d: &Deal, at: &VenueId) -> String {
    match &d.usdt_return {
        Some(p) if p.venue() == at => d.id.clone(),
        _ => format!("{}@{}", d.id, at.0),
    }
}

/// Разбор ключа части: (id сделки, площадка; None — первая часть). Ошибок нет.
pub fn parse(key: &str) -> (&str, Option<VenueId>) {
    match key.rsplit_once('@') {
        Some((id, v)) if !id.is_empty() && !v.is_empty() => (id, Some(VenueId(v.to_string()))),
        _ => (key, None),
    }
}

/// Выручка продаж сделки на площадке до комиссии (Σ исполнено × средняя). Ручной ввод «Продано вручную» в sells не
/// попадает. Принимает: сделку, площадку. Ошибок нет.
pub fn sold_at(d: &Deal, at: &VenueId) -> Decimal {
    d.sells
        .iter()
        .filter(|r| r.request.venue == *at)
        .map(|r| r.filled_qty * r.avg_price)
        .sum()
}

/// Сумма части: выручка продаж на площадке минус их комиссия в котируемой монете этой площадки (решение 118: уходит
/// то, что пришло). Принимает: сделку, площадку, тикер котируемой (None — не задан: без вычета). Может быть ≤ 0.
/// Ошибок нет.
pub fn amount_at(d: &Deal, at: &VenueId, quote: Option<&str>) -> Decimal {
    let fee: Decimal = match quote {
        Some(q) => d
            .sells
            .iter()
            .filter(|r| r.request.venue == *at)
            .map(|r| r.fee_in(q))
            .sum(),
        None => Decimal::ZERO,
    };
    sold_at(d, at) - fee
}

/// Площадки выручки продаж в порядке первой продажи (без повторов). Ошибок нет.
pub fn sell_venues(d: &Deal) -> Vec<VenueId> {
    let mut v: Vec<VenueId> = Vec::new();
    for r in &d.sells {
        if !v.contains(&r.request.venue) {
            v.push(r.request.venue.clone());
        }
    }
    v
}

/// Части возврата по продажам (решение 227): каждая площадка выручки, кроме площадки покупки (там USDT уже дома), с
/// суммой amount_at > 0. Первой идёт площадка, где сделка закрылась (deal.at), если у неё есть часть.
/// Принимает: сделку, котируемую монету площадки (функция: площадка → тикер; None — не задана).
/// Возвращает: [(площадка, сумма)]. Ошибок нет.
pub fn split(d: &Deal, quote: impl Fn(&VenueId) -> Option<String>) -> Vec<(VenueId, Decimal)> {
    let mut out: Vec<(VenueId, Decimal)> = sell_venues(d)
        .into_iter()
        .filter(|v| *v != d.direction.buy_on)
        .map(|v| {
            let a = amount_at(d, &v, quote(&v).as_deref());
            (v, a)
        })
        .filter(|(_, a)| *a > Decimal::ZERO)
        .collect();
    if let Some(i) = out.iter().position(|(v, _)| *v == d.at) {
        let first = out.remove(i);
        out.insert(0, first);
    }
    out
}

/// Продавала ли сделка не только на площадке закрытия (deal.at) — тогда возврат по частям. Ошибок нет.
pub fn multi(d: &Deal) -> bool {
    d.sells.iter().any(|r| r.request.venue != d.at)
}

/// Сумма попытки возврата части (решение 227): ровно сумма части, не свободный баланс площадки. Допуск (решение 227,
/// допуск; Тигран 08.11 «по копейкам — если это меньше одного USDT, то ок»): биржа зачислила на копейки меньше расчёта
/// — свободно меньше части, но не меньше (часть − допуск) → отправляется свободный остаток; допуск только уменьшает
/// сумму, никогда не увеличивает. Принимает: свободно на площадке, сумму части, допуск (None — не задан: строго).
/// Возвращает: сумму к отправке (≤ части). Ошибки: сумма части ≤ 0; свободно меньше части сверх допуска или
/// свободно 0 (чужие USDT не трогаем — ждать или «не вернулись» по решению 163).
pub fn send_amount(free: Decimal, part: Decimal, tol: Option<Decimal>) -> Result<Decimal, String> {
    if part <= Decimal::ZERO {
        return Err(format!("сумма части {part} — нечего возвращать"));
    }
    if free >= part {
        return Ok(part);
    }
    match tol {
        Some(t) if free > Decimal::ZERO && free >= part - t => Ok(free),
        _ => Err(format!(
            "свободно {free}, меньше суммы части {part} — чужие USDT не трогаем, ждём"
        )),
    }
}
