//! Файл: venues/gate/fast_rule.rs
//! Что делает: решение 421 — правила выбора стакана пары Gate для ядра (без состояния): источник отданного стакана,
//! «не старее отданного» при смене потока (по времени биржи t), сверка верха снимка spot.order_book с быстрым стаканом
//! spot.obu (решение 421, ревью). Вынесено из fast_pick.rs (ориентир 200 строк).
//! Кто вызывает: fast_pick.rs.
//! От чего зависит: fast_book.rs (FastBook), core/types (OrderBook).
//! Настройки: нет.
//!
//! «Никогда не старее отданного» (общая постановка решений 420–424, как решение 109): внутри одного потока порядок
//! держат номера (снимки — lastUpdateId в ws_snap.rs, решение 109; быстрый — U = номер + 1). При смене потока номера
//! сравнить нельзя: совпадает ли шкала U/u spot.obu с lastUpdateId spot.order_book, документация не говорит и живой
//! записи spot.obu нет (вопрос в docs/TASKS.md) — поэтому при смене потока стакан отдаётся, только если его время биржи
//! t строго больше t последнего отданного (оба — мс Gate: у spot.order_book «Order book update time in milliseconds», у
//! spot.obu «order book generation timestamp»). Нет t — смены потока нет.
//!
//! Сверка по содержимому (решение 421, ревью; то же правило, что в ревью 420 и 424): номера двух потоков не сравнимы,
//! но время t — одна шкала (мс Gate). Снимок spot.order_book с t строго больше t быстрого стакана показывает стакан
//! после последнего изменения быстрого; spot.obu шлёт изменения не реже, чем раз в 20 / 100 мс («50-level updates every
//! 20 ms», «400-level updates every 100 ms»), снимки — не чаще venue.book_interval (≥ 100 мс). Два таких снимка
//! ПОДРЯД не совпали с верхом быстрого (те уровни, что уходят ядру), а быстрый между ними не менялся (тот же u) —
//! быстрый молчит о реальном изменении дольше периода снимков: расхождение при целой цепочке номеров (например,
//! уровень, вернувшийся в окно 50 / 400 без кадра о нём). Тогда — как разрыв: ядру этот снимок, пересинхронизация.

use crate::core::types::{OrderBook, PriceLevel};

use super::fast_book::FastBook;

/// Источник отданного ядру стакана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// Быстрый поток spot.obu.
    Fast,
    /// Снимки spot.order_book (страховка).
    Snap,
}

/// Последний отданный ядру стакан пары: источник и время биржи t (мс).
pub type Given = Option<(Src, Option<i64>)>;

/// Время биржи стакана, мс. Ошибок нет.
pub fn ms(b: &OrderBook) -> Option<i64> {
    b.exchange_time.map(|d| d.timestamp_millis())
}

/// Можно ли отдать стакан источника src со временем t после уже отданного: тот же источник — да (порядок держат
/// номера потока); другой — только при t строго больше. Ошибок нет.
pub fn allow(given: &Given, src: Src, t: Option<i64>) -> bool {
    match given {
        None => true,
        Some((s, _)) if *s == src => true,
        Some((_, prev)) => matches!((prev, t), (Some(a), Some(b)) if b > *a),
    }
}

/// Сверка снимка с быстрым стаканом (быстрый синхронизирован и отдан ядру последним). Принимает: быстрый стакан, снимок,
/// уровней ядру, u быстрого при прошлом несовпадении (обновляется здесь). Возвращает: true — второй подряд снимок новее
/// быстрого и не равен его верху, а быстрый между ними не менялся (расхождение). Ошибок нет: нет t у снимка или
/// быстрого — сравнить по времени нельзя, расхождение не объявляется.
pub fn diverged(lb: &FastBook, snap: &OrderBook, depth: usize, miss: &mut Option<i64>) -> bool {
    let newer = matches!((ms(snap), lb.time()), (Some(s), Some(f)) if s > f);
    if !newer || same_top(lb, snap, depth) {
        *miss = None;
        return false;
    }
    let id = lb.version();
    if id.is_some() && *miss == id {
        *miss = None;
        return true;
    }
    *miss = id;
    false
}

/// Совпадает ли верх быстрого стакана (depth уровней на сторону) с первыми уровнями снимка. Ошибок нет.
fn same_top(lb: &FastBook, snap: &OrderBook, depth: usize) -> bool {
    let (bids, asks) = lb.top(depth);
    let n = |v: &[PriceLevel]| v.len().min(depth);
    bids[..] == snap.bids[..n(&snap.bids)] && asks[..] == snap.asks[..n(&snap.asks)]
}
