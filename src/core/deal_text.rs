//! Файл: core/deal_text.rs
//! Что делает: человеческие тексты алертов сделки (решение 164): закрытие сделки (формат Тиграна: «✅ Сделка плюсовая» /
//! «❌ Сделка минусовая», план, факт, итог; решение 216 — строки расчёта «без потолка», deal_ceiling.rs), остановка
//! сделки, шаг сделки по-русски для хронологии.
//! Решение 236: строка «Возврат USDT» — затраты возврата в факте или почему их нет (deal_return_cost.rs).
//! Решение 251: монета, путь и где сейчас — карточкой сделки (deal_card.rs), суммы — блоком «Цифры» (notify/alert.rs).
//! Логику не меняет — только то, что видит человек в Telegram и на экранах.
//! Кто вызывает: ledger.rs (deal.closed), ledger_load.rs, manual_feed.rs, другие места с текстом сделки.
//! От чего зависит: deal.rs, deal_ceiling.rs (строки «без потолка»), deal_return_cost.rs, day_pair.rs (подпись пары), notify/human.rs (формат чисел, Msg).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::notify::human::{self as h, Msg};

use super::deal::{Deal, Step};
use super::deal_card::card;
use super::deal_ceiling::Ceiling;

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
/// «Беру на себя» — что случилось, где монеты и причина. Принимает: сделку (уже с шагом и итогом), причину, расчёт
/// «без потолка» этой сделки (решение 216, deal_ceiling::read; None — строк «без потолка» нет; у остановленной и
/// переданной человеку не показывается).
pub fn closed(d: &Deal, why: &str, ceil: Option<&Ceiling>) -> String {
    match d.step {
        Step::Closed => closed_result(d, why, ceil),
        Step::TakenOver => Msg::new(h::INFO, "Сделка передана человеку")
            .card(card(d))
            .kv("Вложено", h::usd(d.invested))
            .id(&d.id)
            .render(),
        _ => Msg::new(h::STOP, "Сделка остановлена")
            .card(card(d))
            .kv("Вложено", h::usd(d.invested))
            .reason(why)
            .todo("разобрать сделку на экране «Ручное управление»")
            .id(&d.id)
            .render(),
    }
}

/// Закрытая сделка — формат Тиграна (решение 164): заголовок по знаку факта, монета и маршрут, размер ордера (решение
/// 191), план, факт, итог;
/// способ закрытия — короткой строкой, если продано вручную или есть продажа; строки «без потолка» (решение 216).
/// Принимает: сделку, причину, расчёт «без потолка» (None — без этих строк).
fn closed_result(d: &Deal, why: &str, ceil: Option<&Ceiling>) -> String {
    let fact = fact(d);
    let plus = fact >= Decimal::ZERO;
    let mut m = if plus {
        Msg::new(h::OK, "Сделка плюсовая")
    } else {
        Msg::new(h::LOSS, "Сделка минусовая")
    };
    // Решение 191: размер ордера сделки — сколько вложено в покупку.
    m = m
        .card(card(d))
        .kv("Размер ордера", h::usd(d.invested))
        .kv("План", h::signed_usd(d.plan_profit_usd))
        .kv("Факт", h::signed_usd(fact));
    m = if plus {
        m.kv("Заработали", h::usd(fact))
    } else {
        m.kv("Итого минус", h::usd(fact))
    };
    if let Some(r) = return_line(d) {
        m = m.kv("Возврат USDT", r);
    }
    if let Some(how) = how_closed(d, why) {
        m = m.kv("Закрыта", how);
    }
    if let Some(c) = ceil {
        m = super::deal_ceiling::lines(m, c);
    }
    m.id(&d.id).render()
}

/// Решение 236: строка о затратах возврата USDT в итоге: учтённые затраты («вернулись») и части без известной
/// стоимости («не вернулись», «перегнано вручную»). Принимает: сделку. Возвращает: None — возврата нет. Ошибок нет.
pub fn return_line(d: &Deal) -> Option<String> {
    use super::deal_return_cost as rc;
    let unknown = rc::unknown_parts(d);
    let cost = d.return_cost_usd.filter(|c| !c.is_zero());
    let mut parts = vec![];
    if let Some(c) = cost {
        parts.push(format!("−{} (комиссия вывода, газ) — в факте", h::usd(c)));
    }
    if rc::in_progress(d) {
        parts.push("идёт — затраты войдут в итог по факту".into());
    }
    if unknown > 0 {
        parts.push(
            "не вернулись / перегнано вручную без суммы зачисления — стоимость неизвестна, в факт не вошла".into(),
        );
    }
    (!parts.is_empty()).then(|| parts.join("; "))
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
        // Решение 225: софтовый «Стоп» снимается сам с новым днём (day_roll.rs).
        .line("Снимется сама с новым днём (UTC)")
        .todo("закрыть сделки на экране «Ручное управление», затем «Сбросить лимит дня» и «Старт»")
        .render()
}

/// Решение 258 (было — пауза площадки, решение 173): пауза одного направления одной биржи по дневному лимиту пары
/// (risk.venue_daily_loss). Принимает: id пары (day_pair.rs), причину паузы из daily_venue_limit.rs («DEX → MEXC:
/// убыток за день … больше лимита …»). «DEX ↔ DEX» — на паузе только циклы, направления бирж с DEX торгуют.
pub fn pair_daily_stop(pair: &str, why: &str) -> String {
    let l = super::day_pair::label(pair);
    let cycles =
        super::day_pair::kind_of(pair) == Some(crate::settings::venue_dir::DirKind::DexDex);
    let (head, rest) = if cycles {
        (
            "Циклы DEX ↔ DEX на паузе: дневной лимит убытка".to_string(),
            "Биржи ↔ DEX и остальные направления торгуют; идущие сделки доводятся",
        )
    } else {
        (
            format!("Направление {l} на паузе: дневной лимит убытка"),
            "Другие направления этой биржи и остальные площадки торгуют; идущие сделки доводятся",
        )
    };
    Msg::new(h::STOP, head)
        .line(why)
        .line(rest)
        .line("Снимется сама с новым днём (UTC)")
        .todo(format!(
            "разобрать сделки на экране «Ручное управление», затем «Снять паузу» или «Сбросить лимит дня» по {l}"
        ))
        .render()
}

/// Сделка прервана перезапуском (risk.stop при запуске). Принимает: сделку (до смены шага на Halted).
pub fn interrupted(d: &Deal) -> String {
    Msg::new(h::STOP, "Сделка прервана перезапуском бота")
        .card(card(d))
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
