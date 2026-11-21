//! Файл: core/book_still.rs
//! Что делает: решение 317 (доработка) — отметки последнего ИЗМЕНЕНИЯ содержимого стакана биржи в состоянии рынка
//! ядра (market_state.rs), отдельно от received_at кадра: кадр с тем же содержимым отметку не двигает. По каждой
//! стороне стакана хранится время последнего изменения каждого префикса уровней (уровни 0..=i от лучшей цены): новый
//! кадр, впервые отличающийся от прошлого на уровне k (цена или количество; появление, исчезновение или сдвиг уровня —
//! тоже отличие по позиции), двигает отметки префиксов длиной > k на время получения кадра; префиксы короче k не
//! менялись. Так «уровни, которые использует котировка на объёме сделки» (первые n уровней, покрывающие объём) стоят
//! с отметки префикса n. Адаптеры шлют стакан целиком (снимок потоком — MEXC, решение 109) — сравнение содержимого
//! отличает «тот же стакан» от «изменился» без знания площадки. Страховка (ревью 16.11): кадр позже прошлого больше
//! чем на порог свежести стакана площадки (venue.book_stale_ms) — отсчёт заново.
//!
//! Кто вызывает: core/book_marks_on.rs note (из market_state.rs на каждый стакан — только при включённом признаке
//! отметок), core/market_read.rs (book_still_since).
//! От чего зависит: core/types (OrderBook, PriceLevel, Side), chrono, rust_decimal.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{OrderBook, PriceLevel, Side};

/// Отметки изменения стакана одной пары: по стороне — время последнего изменения префикса длиной i + 1.
#[derive(Debug, Clone, Default)]
pub struct BookMarks {
    bids: Vec<DateTime<Utc>>,
    asks: Vec<DateTime<Utc>>,
}

impl BookMarks {
    /// Что делает: отмечает новый кадр стакана. Принимает: прошлый стакан пары в ядре (None — первый или после
    /// сброса: всё «изменилось» в момент кадра), новый стакан, порог свежести стакана площадки в мс
    /// (venue.book_stale_ms; None — не задан). Страховка решения 317: новый кадр пришёл позже прошлого больше чем на
    /// порог — ядро не наблюдало рынок в этом промежутке (потерян «недостоверно», разрыв, пропущенные кадры), отсчёт
    /// заново, как у первого кадра. Ошибок нет.
    pub fn note(&mut self, prev: Option<&OrderBook>, new: &OrderBook, gap_ms: Option<u64>) {
        let at = new.received_at;
        let prev = prev
            .filter(|p| gap_ms.is_none_or(|g| (at - p.received_at).num_milliseconds() <= g as i64));
        let empty: &[PriceLevel] = &[];
        let (ob, oa) = prev.map_or((empty, empty), |p| (&p.bids[..], &p.asks[..]));
        side(&mut self.bids, ob, &new.bids, at);
        side(&mut self.asks, oa, &new.asks, at);
    }

    /// Что делает: с какого момента не менялись уровни стакана, которыми котировка набирает объём.
    /// Принимает: текущий стакан пары, сторону сделки на бирже (покупка — продавцы asks, продажа — покупатели bids),
    /// объём в монетах. Возвращает: время последнего изменения первых n уровней, где n — сколько уровней от лучшей
    /// цены покрывают объём (не покрывают — все уровни стороны: консервативнее); None — уровней нет или отметок
    /// нет. Ошибок нет.
    pub fn still_since(&self, book: &OrderBook, side: Side, qty: Decimal) -> Option<DateTime<Utc>> {
        let (levels, marks) = match side {
            Side::Buy => (&book.asks, &self.asks),
            Side::Sell => (&book.bids, &self.bids),
        };
        let mut cum = Decimal::ZERO;
        let mut n = levels.len();
        for (i, l) in levels.iter().enumerate() {
            cum += l.qty;
            if cum >= qty {
                n = i + 1;
                break;
            }
        }
        n.checked_sub(1).and_then(|i| marks.get(i)).copied()
    }
}

/// Отметки одной стороны. Принимает: отметки, прошлые уровни (первый кадр — пусто), новые уровни, время кадра.
/// Ошибок нет.
fn side(marks: &mut Vec<DateTime<Utc>>, old: &[PriceLevel], new: &[PriceLevel], at: DateTime<Utc>) {
    // Первое отличие по позиции; совпали все общие — длина общей части.
    let k = old
        .iter()
        .zip(new)
        .position(|(a, b)| a != b)
        .unwrap_or(old.len().min(new.len()));
    // Отметки не от прошлого стакана (первый кадр, сброс) — всё с этого кадра.
    let k = if marks.len() == old.len() { k } else { 0 };
    marks.truncate(new.len());
    for m in marks.iter_mut().skip(k) {
        *m = at;
    }
    marks.resize(new.len(), at);
}
