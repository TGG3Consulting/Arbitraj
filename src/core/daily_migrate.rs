//! Файл: core/daily_migrate.rs
//! Что делает: решение 258 — запись итога дня до разбивки по парам (нет поля pair_final: разбивка по площадкам решения
//! 173 — с t91 или до него — или вовсе без разбивки) при загрузке разносится по парам «площадка × тип направления»
//! (day_pair.rs) по сделкам. Владелец (Тигран 12.11): «забудь про сегодня — никаких лимитов в день выкладки не будет» —
//! точность дня выкладки не важна, но без паники и без двойного счёта. Поэтому:
//! - у неразобранных оценок — пара их сделки (Book.deal_pairs);
//! - запись сегодняшняя: факт пары = сумма начислений её сделок в этот день (Deal.day_book, решение 257 — точно; у
//!   сделки без начислений — result_usd закрытой сделки с днём последнего изменения = день записи, как прежде);
//!   каждая сделка — ровно одной паре (двойного счёта нет); разница с общим фактом (поправки разборов прошлых дней,
//!   неточность сделок без начислений) — только в общем итоге, пишется в журнал;
//! - сброс лимита площадки в старой записи (venue_resets) переходит к каждой её паре (у DEX — к «DEX ↔ DEX»): итог пары
//!   в момент сброса = её начисления до отметки — лимит пары считает только убытки после сброса, как было у площадки;
//! - общий итог и общий сброс не меняются; прошлые дни не пересчитываются (их строки daily_result — как записаны).
//!   Паузы по убытку в памяти не хранятся — после перезапуска их ставит заново проверка лимитов пар.
//!
//! Кто вызывает: daily_load.rs (load).
//! От чего зависит: daily.rs (Book), daily_venue.rs (add_final, is_dex), day_pair.rs, deal_book.rs (DayPart), persist.rs,
//! ledger.rs (KIND), deal.rs.
//! Настройки: нет.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::storage::journal::Level;

use super::daily::Book;
use super::day_view::LimitReset;
use super::deal::{Deal, Step};
use super::deal_book::DayPart;
use super::engine::Core;

/// Начисление сделки в итог дня записи: время, сумма, факт (иначе — оценка).
struct Part {
    at: DateTime<Utc>,
    amount: Decimal,
    fact: bool,
}

/// Что делает: разносит книгу дня по парам (см. шапку). Принимает: ядро, день записи, сегодня, запись как JSON (старые
/// поля venue_resets), книгу. Ошибки: сделки не прочитаны — в журнал, книга остаётся без разбивки (общий итог цел).
pub(super) async fn split(
    core: &Core,
    date: NaiveDate,
    today: NaiveDate,
    raw: &Value,
    book: &mut Book,
) {
    let deps = &core.inner.deps;
    let deals = match super::persist::load::<Deal>(&deps.db, super::ledger::KIND).await {
        Ok(d) => d,
        Err(e) => {
            let m = format!(
                "Итог дня {date} не разнесён по направлениям (решение 258): сделки не прочитаны ({e})"
            );
            deps.log(Level::Error, "core.daily", None, m, None).await;
            return;
        }
    };
    let mut by_pair: BTreeMap<String, Vec<Part>> = BTreeMap::new();
    let (mut n, mut guessed, mut sum) = (0usize, 0usize, Decimal::ZERO);
    for d in deals {
        let pair = super::day_pair::of_direction(core, &d.direction);
        if book.provisional.contains_key(&d.id) {
            book.deal_pairs.insert(d.id.clone(), pair.clone());
        }
        if date != today {
            continue;
        }
        let (parts, exact) = parts(&d, date, book);
        if parts.is_empty() {
            continue;
        }
        let fact: Decimal = parts.iter().filter(|p| p.fact).map(|p| p.amount).sum();
        if !fact.is_zero() {
            super::daily_venue::add_final(book, &pair, fact);
        }
        n += 1;
        sum += fact;
        if !exact {
            guessed += 1;
        }
        by_pair.entry(pair).or_default().extend(parts);
    }
    if date == today {
        resets(core, raw, &by_pair, book);
    }
    let rest = book.final_usd - sum;
    let rest = if date != today || rest.is_zero() {
        String::new()
    } else {
        format!(
            "; разница {} USDT (поправки разборов прошлых дней, сделки без начислений) — только в общем итоге",
            rest.round_dp(4)
        )
    };
    let m = format!(
        "Итог дня {date} разнесён по направлениям «площадка × тип» (решение 258): сделок дня {n} (из них по оценке, без \
         начислений решения 257, — {guessed}), их факт {} USDT; общий итог {} USDT не меняется{rest}; прошлые дни не \
         пересчитываются",
        sum.round_dp(4),
        book.final_usd.round_dp(4)
    );
    deps.log(Level::Info, "core.daily", None, m, None).await;
}

/// Сброс лимита площадки старой записи → сброс каждой её пары (итог пары в момент сброса — её начисления до отметки).
/// Принимает: ядро, запись JSON, начисления по парам, книгу. Ошибок нет: неразобранная отметка пропускается.
fn resets(core: &Core, raw: &Value, by_pair: &BTreeMap<String, Vec<Part>>, book: &mut Book) {
    let Some(old) = raw.get("venue_resets").and_then(Value::as_object) else {
        return;
    };
    for (v, r) in old {
        let Ok(r) = serde_json::from_value::<LimitReset>(r.clone()) else {
            continue;
        };
        for p in super::day_pair::of_venue(core, v) {
            let base: Decimal = by_pair
                .get(&p)
                .into_iter()
                .flatten()
                .filter(|x| x.at <= r.at)
                .map(|x| x.amount)
                .sum();
            book.pair_resets.insert(
                p,
                LimitReset {
                    at: r.at,
                    base_usd: base,
                    by: r.by.clone(),
                },
            );
        }
    }
}

/// Что делает: начисления сделки в итог дня `date`. Принимает: сделку, день, книгу (её оценки).
/// Возвращает: (начисления, точно ли — по Deal.day_book). Ошибок нет.
fn parts(d: &Deal, date: NaiveDate, book: &Book) -> (Vec<Part>, bool) {
    if !d.day_book.is_empty() {
        let p = d
            .day_book
            .iter()
            .filter(|e| e.date == date)
            .map(|e| Part {
                at: e.at,
                amount: e.amount,
                fact: e.part == DayPart::Fact,
            })
            .collect();
        return (p, true);
    }
    let mut p = Vec::new();
    if let Some(r) = d.result_usd
        && d.step == Step::Closed
        && d.updated_at.date_naive() == date
    {
        p.push(Part {
            at: d.updated_at,
            amount: r,
            fact: true,
        });
    }
    if let Some((a, day)) = book.provisional.get(&d.id)
        && *day == date
    {
        p.push(Part {
            at: d.updated_at,
            amount: *a,
            fact: false,
        });
    }
    (p, false)
}
