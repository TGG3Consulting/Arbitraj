//! Файл: gateway/core_out_reg.rs
//! Что делает: решение 324 — набор канала ядра по самому каналу (`of`, реэкспорт core_out::of): реестр процесса
//! «канал (WeakSender — канал не держится открытым) → набор»; запись с тем же каналом (Sender::same_channel) или новая
//! при первом обращении; мёртвые записи (все Sender сняты или канал закрыт) выбрасываются.
//! Ревью 324 (Н2): перед реестром — кэш потока (thread_local) уже найденных наборов: повторный `of` того же канала в том
//! же потоке (invalidated бирж на каждое «недостоверно», BookOut::new каждой сессии) — без замка реестра процесса
//! (проверка канала — WeakSender::upgrade + same_channel, атомарные операции без замков). Промах — реестр под замком и
//! запись в кэш потока (мёртвые записи кэша выбрасываются при промахе). Кэш держит клон набора — тот же набор, что в
//! реестре.
//! Кто вызывает: gateway/core_out.rs (реэкспорт of): core_out_book.rs, venues/bsc_dex/pools/emit_pool.rs, тесты.
//! От чего зависит: gateway/core_out.rs (Feed), gateway/events.rs, tokio::sync::mpsc.
//! Настроек нет.

use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};

use tokio::sync::mpsc::{Sender, WeakSender};

use super::core_out::Feed;
use super::events::MarketEvent;

/// Записи «канал → набор».
type Entries = Vec<(WeakSender<MarketEvent>, Feed)>;

thread_local! {
    /// Кэш потока (Н2): наборы, уже найденные этим потоком.
    static NEAR: RefCell<Entries> = const { RefCell::new(Vec::new()) };
}

/// Реестр наборов (один на процесс). Ошибок нет.
fn reg() -> &'static Mutex<Entries> {
    static REG: OnceLock<Mutex<Entries>> = OnceLock::new();
    REG.get_or_init(Mutex::default)
}

/// Запись с тем же каналом. Ошибок нет.
fn find(v: &Entries, out: &Sender<MarketEvent>) -> Option<Feed> {
    v.iter()
        .find(|(w, _)| w.upgrade().is_some_and(|s| s.same_channel(out)))
        .map(|(_, f)| f.clone())
}

/// Запись жива: есть Sender и канал не закрыт. Ошибок нет.
fn alive(w: &WeakSender<MarketEvent>) -> bool {
    w.upgrade().is_some_and(|s| !s.is_closed())
}

/// Набор канала ядра (см. шапку). Принимает: канал. Возвращает: общий набор этого канала. Ошибок нет.
pub fn of(out: &Sender<MarketEvent>) -> Feed {
    if let Some(f) = NEAR.with_borrow(|v| find(v, out)) {
        return f;
    }
    let f = {
        let mut r = reg().lock().unwrap_or_else(|p| p.into_inner());
        r.retain(|(w, _)| alive(w));
        match find(&r, out) {
            Some(f) => f,
            None => {
                let f = Feed::default();
                r.push((out.downgrade(), f.clone()));
                f
            }
        }
    };
    NEAR.with_borrow_mut(|v| {
        v.retain(|(w, _)| alive(w));
        v.push((out.downgrade(), f.clone()));
    });
    f
}
