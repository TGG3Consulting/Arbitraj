//! Файл: venues/kucoin/fast_check.rs
//! Что делает: решение 423 — сверка быстрого стакана пары (local_book.rs) с кадром снимков /spotMarket/level2Depth5|50
//! по времени биржи: снимок с timestamp T сравнивается (первые venue.depth_levels уровней, точно) с состояниями
//! локального стакана, у которых время последнего применённого кадра изменений равно T. Расхождение — только если
//! состояние с временем T было, ни одно не совпало, и локальный стакан ушёл дальше T. Нельзя сопоставить (локальный
//! стакан уже дальше T, когда пришёл снимок; или перескочил через T без состояния с этим временем; времени нет) — сверки
//! нет, расхождением это не считается.
//! Кто вызывает: ws_fast.rs (на каждом изменении локального стакана и на каждом кадре снимков быстрой пары), тесты.
//! От чего зависит: gateway/feed_health.rs (BookPrint).
//! Настройки: нет.
//!
//! Основание сопоставления — ФАКТ ЖИВОЙ ЗАПИСИ, не документации: в tests/data/cex_samples_2026_11_04/kucoin_ws.jsonl у
//! каждого из 11 кадров level2Depth50 поле timestamp равно time последнего пришедшего перед ним кадра /market/level2
//! той же пары, и первые 5 уровней снимка совпадают с книгой после этого кадра (10 из 10 сверок от первого снимка;
//! тест tests/kucoin_fast_book.rs). Документация
//! (https://www.kucoin.com/docs-new/3470070w0) называет поле только «timestamp»; связь с time изменений не описана —
//! вопрос в docs/TASKS.md. Несколько кадров изменений с одним time — снимок мог быть снят после любого из них, поэтому
//! хранятся отпечатки всех состояний с последним временем.

use crate::gateway::feed_health::BookPrint;

/// Итог сверки.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Совпало.
    Ok,
    /// Ждём состояния локального стакана с временем снимка.
    Wait,
    /// Сопоставить нельзя или нечего сверять.
    Skip,
    /// Расхождение (причина).
    Diverge(String),
}

/// Снимок, ждущий сопоставления.
#[derive(Debug, Clone)]
struct Want {
    at: i64,
    print: BookPrint,
    /// Было ли состояние локального стакана с временем at (сравнение было и не совпало).
    seen: bool,
}

/// Сверка одной пары.
#[derive(Debug, Default, Clone)]
pub struct Check {
    /// Отпечатки состояний с последним временем биржи: (время, отпечатки по порядку).
    recent: Option<(i64, Vec<BookPrint>)>,
    want: Option<Want>,
}

impl Check {
    /// Что делает: новое состояние локального стакана. Принимает: время его последнего кадра изменений, отпечаток
    /// верхних уровней. Возвращает: Verdict (Diverge — снимок с временем T не совпал ни с одним состоянием T, а стакан
    /// ушёл дальше). Ошибок нет.
    pub fn on_local(&mut self, time: i64, print: BookPrint) -> Verdict {
        match self.recent.as_mut() {
            Some((t, prints)) if *t == time => prints.push(print.clone()),
            _ => self.recent = Some((time, vec![print.clone()])),
        }
        let Some(w) = self.want.as_mut() else {
            return Verdict::Skip;
        };
        if time < w.at {
            return Verdict::Wait;
        }
        if time == w.at {
            w.seen = true;
            if print == w.print {
                self.want = None;
                return Verdict::Ok;
            }
            return Verdict::Wait;
        }
        let w = self.want.take().expect("проверено выше");
        if !w.seen {
            return Verdict::Skip;
        }
        Verdict::Diverge(describe(&w))
    }

    /// Что делает: кадр снимков быстрой пары. Принимает: timestamp снимка (мс), отпечаток его верхних уровней, время
    /// последнего кадра изменений локального стакана (None — нет). Прежний несопоставленный снимок заменяется новым.
    /// Возвращает: Ok, Wait или Skip. Ошибок нет.
    pub fn on_snapshot(&mut self, at: i64, print: BookPrint, local: Option<i64>) -> Verdict {
        self.want = None;
        let Some(lt) = local else {
            return Verdict::Skip;
        };
        if lt > at {
            return Verdict::Skip;
        }
        let seen = lt == at;
        if seen
            && let Some((t, prints)) = &self.recent
            && *t == at
            && prints.contains(&print)
        {
            return Verdict::Ok;
        }
        self.want = Some(Want { at, print, seen });
        Verdict::Wait
    }

    /// Сбрасывает сверку (стакан пересинхронизируется). Ошибок нет.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Текст расхождения: время снимка и его лучшие цены. Ошибок нет.
fn describe(w: &Want) -> String {
    let best = |l: &[crate::core::types::PriceLevel]| {
        l.first()
            .map(|l| format!("{}×{}", l.price, l.qty))
            .unwrap_or_else(|| "—".into())
    };
    format!(
        "кадр снимков с timestamp {} (лучшие: bid {}, ask {}) не совпал с быстрым стаканом этого времени",
        w.at,
        best(&w.print.bids),
        best(&w.print.asks)
    )
}
