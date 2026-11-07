//! Файл: core/deal_ceiling.rs
//! Что делает: решение 216 — строки расчёта «без потолка» (решение 141) в сообщении Telegram о закрытой сделке
//! (deal.closed). «Потолок» по Тиграну — сколько можно было вложить без ограничения нашего бюджета (opt.invested записи
//! без потолка), а не наш бюджет. Плюсовая: «💸 Упустили» (missed) и «🔝 Потолок: вложить → прибыль» (opt.invested,
//! opt.profit; оптимум не проходит пороги — пометка «(не проходит пороги)»). Минусовая: «🔝 Потолок» и «💥 Через
//! потолок потеряли бы ≈ …» — оценка тем же процентом убытка на объёме потолка. Записи нет — «Без потолка: не
//! посчитано» и причина, если она известна в момент закрытия (выключено, цикл DEX ↔ DEX, ещё считается / ждёт в
//! очереди, сбой чтения базы).
//! Чтение — ОДИН запрос к SQLite (storage/uncapped_db.rs by_deal, последняя запись сделки по её id); очередь расчёта
//! не ждётся — уведомление не задерживается.
//! Кто вызывает: core/ledger.rs (finish: of_closed → deal_text::closed), tests/notify_human.rs.
//! От чего зависит: storage/uncapped_db.rs, uncapped_state.rs (слоты очереди), size_uncapped.rs (At), deal.rs,
//! notify/human.rs (формат чисел, Msg), settings.
//! Настройки: uncapped.enabled (только причина «не посчитано»).

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::notify::human::{self as h, Msg};
use crate::settings::read::At as SAt;
use crate::storage::uncapped_db;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::size_uncapped::At;
use super::uncapped_state::Key;

/// Часть записи «без потолка» (core/uncapped_types.rs Record, JSON в SQLite), нужная сообщению.
#[derive(Debug, Clone, Deserialize)]
struct Part {
    opt: At,
    opt_passes: bool,
    missed: Decimal,
}

/// Расчёт «без потолка» сделки для сообщения.
#[derive(Debug, Clone, PartialEq)]
pub enum Ceiling {
    /// Запись есть: оптимум без потолка — вложить `invested` → прибыль `profit`; `passes` — проходит ли он пороги
    /// money.min_profit_usd / pct; `missed` — упущено, USDT.
    Known {
        invested: Decimal,
        profit: Decimal,
        passes: bool,
        missed: Decimal,
    },
    /// Записи нет (или не читается): причина, если известна.
    Unknown(Option<String>),
}

/// Разбор JSON записи. Принимает: тело строки uncapped. Возвращает: Known или Unknown с причиной «запись не читается».
pub fn parse(body: &str) -> Ceiling {
    match serde_json::from_str::<Part>(body) {
        Ok(p) => Ceiling::Known {
            invested: p.opt.invested,
            profit: p.opt.profit,
            passes: p.opt_passes,
            missed: p.missed,
        },
        Err(e) => Ceiling::Unknown(Some(format!("запись не читается: {e}"))),
    }
}

/// Для сообщения о закрытии: у закрытой (шаг Closed) — read, у остановленной и переданной человеку — None (строк нет,
/// чтения нет). Принимает: ядро, сделку. Ошибок нет.
pub async fn of_closed(core: &Core, d: &Deal) -> Option<Ceiling> {
    if d.step != Step::Closed {
        return None;
    }
    Some(read(core, d).await)
}

/// Расчёт «без потолка» сделки — одно чтение последней записи по id сделки, без ожидания очереди.
/// Принимает: ядро, сделку. Возвращает: Ceiling. Ошибок нет — сбой базы становится причиной «не посчитано».
pub async fn read(core: &Core, d: &Deal) -> Ceiling {
    match uncapped_db::by_deal(&core.inner.deps.db, d.id.clone()).await {
        Ok(Some((_, body))) => parse(&body),
        Ok(None) => Ceiling::Unknown(why_none(core, d)),
        Err(e) => Ceiling::Unknown(Some(format!("сбой чтения базы: {e}"))),
    }
}

/// Почему записи нет — только то, что известно наверняка в момент закрытия. Сбой самого расчёта по сделке не хранится
/// (uncapped_store::failed пишет только общий счётчик и последнюю причину на экране «Упущено из-за потолка») — тогда
/// None. Принимает: ядро, сделку. Ошибок нет.
fn why_none(core: &Core, d: &Deal) -> Option<String> {
    // Цикл DEX ↔ DEX (buy_on = sell_on, core/cycle_deal.rs): расчёт без потолка ставит только executor.rs.
    if d.direction.buy_on == d.direction.sell_on {
        return Some("для циклов DEX ↔ DEX не считается".into());
    }
    match core
        .inner
        .deps
        .settings
        .flag("uncapped.enabled", SAt::default())
    {
        Ok(true) => {}
        Ok(false) => return Some("расчёт выключен (uncapped.enabled = нет)".into()),
        Err(e) => return Some(e.message),
    }
    let slots = lk(&core.inner.uncapped.slots);
    match slots.get(&Key::Deal(d.id.clone())) {
        Some(s) if s.running => Some("ещё считается".into()),
        Some(s) if s.pending.is_some() => Some("ждёт в очереди расчёта".into()),
        _ => None,
    }
}

/// Строки «без потолка» в сообщение закрытой сделки. Принимает: сообщение, сделку, её факт (итог, USDT), знак
/// (плюсовая — итог ≥ 0), расчёт. Возвращает: сообщение со строками. Ошибок нет.
pub fn lines(m: Msg, d: &Deal, fact: Decimal, plus: bool, c: &Ceiling) -> Msg {
    let (invested, profit, passes, missed) = match c {
        Ceiling::Known {
            invested,
            profit,
            passes,
            missed,
        } => (*invested, *profit, *passes, *missed),
        Ceiling::Unknown(why) => {
            let tail = why.as_ref().map(|w| format!(" ({w})")).unwrap_or_default();
            return m.line(format!("Без потолка: не посчитано{tail}"));
        }
    };
    let cap = format!("🔝 Потолок: {}", h::usd(invested));
    if plus {
        let mark = if passes {
            ""
        } else {
            " (не проходит пороги)"
        };
        return m
            .line(format!("💸 Упустили: {}", h::usd(missed)))
            .line(format!("{cap} → прибыль {} USDT{mark}", h::money(profit)));
    }
    let m = m.line(cap);
    match loss_at(fact, d.invested, invested) {
        Some(x) => m.line(format!("💥 Через потолок потеряли бы ≈ {}", h::usd(x))),
        None => m,
    }
}

/// Оценка убытка на объёме потолка: факт / вложено × вложение потолка — тот же процент убытка, что у сделки.
/// Это ОЦЕНКА, не расчёт: продажи на объёме потолка не было; на большем объёме покупка шла бы глубже по стакану или
/// пулу (средняя цена хуже), продажа — тоже глубже, а газ, комиссия вывода и другие постоянные затраты на крупном
/// объёме весят меньше. Поэтому в сообщении — знак «≈». Принимает: факт сделки, вложено, вложение потолка.
/// Возвращает: оценку (со знаком факта); вложено 0 — None (процента нет). Ошибок нет.
pub fn loss_at(fact: Decimal, invested: Decimal, cap: Decimal) -> Option<Decimal> {
    (!invested.is_zero()).then(|| fact / invested * cap)
}
