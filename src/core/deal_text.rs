//! Файл: core/deal_text.rs
//! Что делает: человеческие тексты алертов сделки (решение 164): закрытие сделки (формат Тиграна: «✅ Сделка плюсовая» /
//! «❌ Сделка минусовая», план, факт, итог), остановка сделки, шаг сделки по-русски для хронологии.
//! Логику не меняет — только то, что видит человек в Telegram и на экранах.
//! Кто вызывает: ledger.rs (deal.closed), ledger_load.rs, manual_feed.rs, другие места с текстом сделки.
//! От чего зависит: deal.rs, notify/human.rs (формат чисел, Msg).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::notify::human::{self as h, Msg};

use super::deal::{Deal, Step};

/// Шаг сделки по-русски (для хронологии и экранов). Принимает: шаг.
pub fn step_name(s: Step) -> &'static str {
    match s {
        Step::Buying => "покупка",
        Step::Transfer => "перевод",
        Step::Selling => "продажа",
        Step::Stuck => "монета застряла",
        Step::WaitingHuman => "ждёт решения человека",
        Step::Closed => "закрыта",
        Step::Halted => "остановлена",
        Step::TakenOver => "монеты у человека",
    }
}

/// Строка «монета откуда → куда». Принимает: сделку.
pub fn coin_route(d: &Deal) -> String {
    let x = &d.direction;
    format!("{} {}", x.asset.ticker, h::route(&x.buy_on.0, &x.sell_on.0))
}

/// Строка «монета: количество на площадке». Принимает: сделку.
pub fn coins_at(d: &Deal) -> String {
    format!(
        "{} {} на {}",
        h::qty(d.qty),
        d.direction.asset.ticker,
        h::venue(&d.at.0)
    )
}

/// Текст закрытия сделки для Telegram (deal.closed). Закрыта — плюсовая / минусовая по факту; остановлена или
/// «Беру на себя» — что случилось, где монеты и причина. Принимает: сделку (уже с шагом и итогом), причину.
pub fn closed(d: &Deal, why: &str) -> String {
    match d.step {
        Step::Closed => closed_result(d, why),
        Step::TakenOver => Msg::new(h::INFO, "Сделка передана человеку")
            .line(coin_route(d))
            .kv("Монеты", coins_at(d))
            .kv("Вложено", h::usd(d.invested))
            .id(&d.id)
            .render(),
        _ => Msg::new(h::STOP, "Сделка остановлена")
            .line(coin_route(d))
            .kv("Монеты", coins_at(d))
            .kv("Вложено", h::usd(d.invested))
            .reason(why)
            .todo("разобрать сделку на экране «Ручное управление»")
            .id(&d.id)
            .render(),
    }
}

/// Закрытая сделка — формат Тиграна (решение 164): заголовок по знаку факта, монета и маршрут, размер ордера (решение
/// 191), план, факт, итог;
/// способ закрытия — короткой строкой, если продано вручную или есть продажа. Принимает: сделку, причину.
fn closed_result(d: &Deal, why: &str) -> String {
    let fact = fact(d);
    let plus = fact >= Decimal::ZERO;
    let mut m = if plus {
        Msg::new(h::OK, "Сделка плюсовая")
    } else {
        Msg::new(h::LOSS, "Сделка минусовая")
    };
    // Решение 191: размер ордера сделки — сколько вложено в покупку.
    m = m
        .line(coin_route(d))
        .kv("Размер ордера", h::usd(d.invested))
        .kv("План", h::signed_usd(d.plan_profit_usd))
        .kv("Факт", h::signed_usd(fact));
    m = if plus {
        m.kv("Заработали", h::usd(fact))
    } else {
        m.kv("Итого минус", h::usd(fact))
    };
    if let Some(how) = how_closed(d, why) {
        m = m.kv("Закрыта", how);
    }
    m.id(&d.id).render()
}

/// Факт закрытой сделки: итог из учёта, иначе выручка − вложено − затраты. Принимает: сделку. Ошибок нет.
fn fact(d: &Deal) -> Decimal {
    d.result_usd.unwrap_or(d.proceeds - d.invested - d.spent)
}

/// Решение 176: исход закрытия для маршрута чатов (notify/routes.rs) — тот же знак, что у заголовка «плюсовая» /
/// «минусовая». Принимает: сделку. Возвращает: Some(true) — закрыта с итогом ≥ 0, Some(false) — с итогом < 0,
/// None — не закрыта (остановлена, у человека). Ошибок нет.
pub fn outcome(d: &Deal) -> Option<bool> {
    (d.step == Step::Closed).then(|| fact(d) >= Decimal::ZERO)
}

/// Способ закрытия коротко: «продано вручную» или «продажей на DEX». Нет продажи — None.
fn how_closed(d: &Deal, why: &str) -> Option<String> {
    if why.contains("вручную") {
        return Some("продано вручную".into());
    }
    (!d.sells.is_empty()).then(|| format!("продажей на {}", h::venue(&d.at.0)))
}

/// Остановка всей торговли по общему дневному лимиту (risk.daily_loss, risk.max_daily_loss_total_usd).
/// Принимает: текст лимита из daily_limit.rs.
pub fn daily_stop(stop: &str) -> String {
    Msg::new(h::STOP, "Торговля остановлена")
        .line(cap(stop))
        .todo("закрыть сделки на экране «Ручное управление», затем «Сбросить лимит дня»")
        .render()
}

/// Решение 173: пауза одной площадки по её дневному лимиту (risk.venue_daily_loss). Принимает: id площадки, причину
/// паузы из daily_venue_limit.rs («MEXC: убыток за день … больше лимита …»).
/// cycles — это DEX: на паузе только циклы DEX ↔ DEX, направления бирж с DEX торгуют.
pub fn venue_daily_stop(venue: &str, why: &str, cycles: bool) -> String {
    let v = h::venue(venue);
    let (head, rest) = if cycles {
        (
            format!("Циклы {v} ↔ {v} на паузе: дневной лимит убытка"),
            "Биржи ↔ DEX и остальные площадки торгуют; идущие сделки доводятся",
        )
    } else {
        (
            format!("Торговля на {v} на паузе: дневной лимит убытка"),
            "Остальные площадки торгуют; идущие сделки доводятся",
        )
    };
    Msg::new(h::STOP, head)
        .line(why)
        .line(rest)
        .todo(format!(
            "разобрать сделки {v} на экране «Ручное управление», затем «Снять паузу» или «Сбросить лимит дня» по {v}"
        ))
        .render()
}

/// Сделка прервана перезапуском (risk.stop при запуске). Принимает: сделку (до смены шага на Halted).
pub fn interrupted(d: &Deal) -> String {
    Msg::new(h::STOP, "Сделка прервана перезапуском бота")
        .line(coin_route(d))
        .kv("Монеты", coins_at(d))
        .kv("Была на шаге", step_name(d.step))
        .todo("проверить балансы и разобрать сделку на экране «Ручное управление»")
        .id(&d.id)
        .render()
}

/// Первая буква — заглавная. Принимает: текст.
pub fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
