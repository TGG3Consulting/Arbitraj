//! Файл: core/book_marks_on.rs
//! Что делает: решение 317 (ревью 16.11, нагрузка) — признак «отметки изменения стакана нужны» в состоянии рынка ядра
//! (market_state.rs marks_on). Отметки (book_still.rs) нужны только правилу 317 (signal.dex_event_skip_hold) и записи
//! замера core.signal_hold (log.signal_hold, поле cex_still_ms). Обе не заданы или «нет» — признак выключен: на кадр
//! стакана ядро не делает НИЧЕГО сверх одной проверки признака (ни ключа, ни поиска, ни сравнения уровней), отметки
//! очищены — нагрузка как до решения 317. Включили — отсчёт с первого кадра после включения (отметки моложе стакана —
//! book_still.rs side: k = 0, безопасно). Признак обновляется по рассылке изменений хранилища настроек (как
//! rebuild.rs watch_settings, feed_watch.rs): подписка — до чтения, отставание подписчика — перечитать всё.
//!
//! Кто вызывает: core/engine.rs (refresh до запуска приёма событий, задача watch), core/market_state.rs (note — на кадр
//! стакана при включённом признаке).
//! От чего зависит: engine.rs (Core, wr), market_state.rs (MarketState), book_still.rs (BookMarks), settings.
//! Страховка (ревью 16.11): пороги свежести стакана бирж (venue.book_stale_ms) — для отсчёта заново после разрыва
//! между кадрами (book_still.rs note); перечитываются при изменении этой настройки.
//! Настройки: signal.dex_event_skip_hold, log.signal_hold (признак), venue.book_stale_ms (порог разрыва).

use std::collections::HashMap;

use tokio::sync::broadcast::error::RecvError;

use crate::core::types::{OrderBook, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::book_still::BookMarks;
use super::engine::{Core, rd, wr};
use super::market_state::MarketState;

/// Настройки, от которых зависит признак.
const NAMES: [&str; 2] = ["signal.dex_event_skip_hold", "log.signal_hold"];

/// Порог свежести стакана площадки (страховка решения 317: разрыв между кадрами больше — отсчёт заново).
const GAP: &str = "venue.book_stale_ms";

/// Что делает: читает настройки признака и ставит его в состояние рынка; при включённом — пороги свежести стакана
/// бирж (venue.book_stale_ms по площадке; не задан — порога нет: котировка биржи без него и так отказывает, решение
/// 238). Принимает: ядро. Ошибок нет (не задана — «нет»).
pub fn refresh(core: &Core) {
    let s = &core.inner.deps.settings;
    let on = NAMES
        .iter()
        .any(|n| s.flag(n, At::default()).unwrap_or(false));
    let gaps: HashMap<VenueId, u64> = if on {
        rd(&core.inner.kinds)
            .iter()
            .filter(|(_, k)| *k == VenueKind::Cex)
            .filter_map(|(v, _)| {
                let at = At {
                    venue: Some(&v.0),
                    coin: None,
                };
                Some((v.clone(), s.integer(GAP, at).ok()?))
            })
            .collect()
    } else {
        HashMap::new()
    };
    wr(&core.inner.market).set_book_marks(on, gaps);
}

/// Задача: признак по изменениям настроек. Работает, пока жива рассылка настроек. Ошибок нет.
pub async fn watch(core: Core) {
    // Подписка — до чтения: изменение между чтением и ожиданием не теряется.
    let mut rx = core.inner.deps.settings.subscribe();
    refresh(&core);
    loop {
        match rx.recv().await {
            Ok(c) if NAMES.contains(&c.name.as_str()) || c.name == GAP => refresh(&core),
            Ok(_) => {}
            // Отстали — нужное изменение могло пропасть: перечитать.
            Err(RecvError::Lagged(_)) => refresh(&core),
            Err(RecvError::Closed) => return,
        }
    }
}

impl MarketState {
    /// Что делает: включает или выключает отметки изменения стакана. Выключено — отметки очищены, на кадр стакана
    /// они не считаются. Принимает: признак, пороги свежести стакана по площадке (venue.book_stale_ms; нет площадки —
    /// порога нет). Ошибок нет.
    pub fn set_book_marks(&mut self, on: bool, gaps: HashMap<VenueId, u64>) {
        self.marks_on = on;
        self.book_gap_ms = gaps;
        if !on {
            self.book_marks.clear();
        }
    }

    /// Включены ли отметки изменения стакана (решение 317). Ошибок нет.
    pub fn book_marks_on(&self) -> bool {
        self.marks_on
    }
}

/// Решение 317: отметки изменения стакана пары (book_still.rs) — только при включённом признаке. Принимает: отметки,
/// ключ пары, прошлый стакан пары (None — первый), новый стакан, порог свежести стакана площадки. Ошибок нет.
pub(super) fn note(
    marks: &mut HashMap<(VenueId, String), BookMarks>,
    key: &(VenueId, String),
    prev: Option<&OrderBook>,
    new: &OrderBook,
    gap_ms: Option<u64>,
) {
    match marks.get_mut(key) {
        Some(m) => m.note(prev, new, gap_ms),
        None => {
            let mut m = BookMarks::default();
            m.note(prev, new, gap_ms);
            marks.insert(key.clone(), m);
        }
    }
}
