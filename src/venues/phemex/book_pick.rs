//! Файл: venues/phemex/book_pick.rs
//! Что делает: решение 427 — выбор стакана пары между основным потоком и запасным (страховка в памяти, чистая логика
//! без окружения). Основной поток — orderbook.subscribe [symbol] (depth 30, 20 мс) на соединениях market-N; запасной
//! (venue.book_reserve) — та же пара на отдельном соединении market-N-reserve. Каждый поток ведёт свой локальный стакан
//! (local_book.rs). Ядру отдаётся стакан основного потока, пока он синхронизирован; разрыв sequence, неразборчивый
//! кадр, отказ подписки или обрыв основного соединения — сразу последний стакан запасного потока (если он
//! синхронизирован), основной ждёт снимок (≤ 60 с) и после него снова главный. Ни один из потоков не синхронизирован —
//! стакан недостоверен (как до решения 427).
//! Никогда не отдаётся стакан старее уже отданного: порядок — по `timestamp` кадра (наносекунды, часы биржи; одна шкала
//! у обоих потоков — поле одного и того же сообщения OrderBook). Номера sequence двух соединений не сравниваются: что
//! они общие, документация не говорит. Кадр с тем же timestamp отдаётся (та же версия; ревью 427: контрольный снимок с
//! тем же sequence, исправивший стакан, иначе не дошёл бы до ядра).
//! Ревью 427: запись в память и отдача ядру — внутри замка пары (frame_with / lost_with), иначе два соединения пары
//! могли отдать стаканы ядру не по порядку timestamp.
//! Без запасного потока (venue.book_reserve не задана) — поведение до решения 427: каждый кадр основного потока отдаётся,
//! разрыв — стакан недостоверен.
//! Кто вызывает: book_lane.rs (по кадру и по потере потока), тесты.
//! От чего зависит: core/types (OrderBook), book.rs (Alive).
//! Настройки: нет (наличие запасного потока передаёт market.rs).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::core::types::OrderBook;

use super::book::Alive;

/// Поток стакана пары.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Основной: orderbook.subscribe [symbol] на market-N.
    Main = 0,
    /// Запасной: venue.book_reserve на market-N-reserve.
    Reserve = 1,
}

impl Role {
    /// Другой поток. Ошибок нет.
    fn other(self) -> Role {
        match self {
            Role::Main => Role::Reserve,
            Role::Reserve => Role::Main,
        }
    }
}

/// Событие для счёта и журнала (book_lane.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Основной поток пары потерян (разрыв, неразборчивый кадр, отказ подписки, обрыв соединения).
    MainGap,
    /// Запасной поток пары потерян.
    ReserveGap,
    /// Ядру переключён запасной поток.
    ToReserve,
    /// Основной снова синхронизирован (снимок) и снова главный после запасного.
    ToMain,
    /// Основной снова синхронизирован, запасной в это время не служил.
    MainResync,
    /// Запасной снова синхронизирован.
    ReserveResync,
    /// Ни один поток не синхронизирован — стакан недостоверен (только при запасном потоке).
    Blind,
    /// Кадр без timestamp при запасном потоке — не с чем сравнить, ядру не отдан.
    NoTime,
}

/// Стакан потока, готовый к отдаче: стакан, живость его соединения, время приёма кадра (unix мс).
#[derive(Clone)]
pub struct Taken {
    pub book: OrderBook,
    pub alive: Alive,
    pub seen_ms: i64,
}

/// Что сделать при потере потока.
pub enum Out {
    /// Отдать ядру стакан другого потока (он новее отданного).
    Give(Taken),
    /// Стакан в памяти остаётся (он новейший верный), живость — по соединению другого потока.
    Keep(Alive),
    /// Стакан недостоверен.
    Invalidate,
    /// Ничего: этот поток ядру не служил.
    Nothing,
}

/// Состояние пары. Индекс массивов — Role.
#[derive(Default)]
struct St {
    ok: [bool; 2],
    lost: [bool; 2],
    last: [Option<(Taken, i64)>; 2],
    given_ns: Option<i64>,
    src: Option<Role>,
}

/// Выбор стакана всех пар одной подписки. Дешёво клонируется; замок — на пару (спорят только два соединения пары).
#[derive(Clone)]
pub struct Pick {
    reserve: bool,
    map: Arc<HashMap<String, Mutex<St>>>,
}

impl Pick {
    /// Принимает: тикеры подписки, есть ли запасной поток. Ошибок нет.
    pub fn new(tickers: &[String], reserve: bool) -> Self {
        let map = tickers
            .iter()
            .map(|t| (t.clone(), Mutex::new(St::default())))
            .collect();
        Self {
            reserve,
            map: Arc::new(map),
        }
    }

    /// Есть ли запасной поток. Ошибок нет.
    pub fn reserve(&self) -> bool {
        self.reserve
    }

    /// Кадр потока изменил синхронизированный стакан. Принимает: поток, тикер, стакан (верхние уровни), timestamp кадра
    /// (нс), живость соединения, время приёма (unix мс). Возвращает: стакан для ядра (None — не отдавать) и события.
    /// Ошибок нет; неизвестный тикер — отдаётся только без запасного потока.
    pub fn frame(
        &self,
        role: Role,
        ticker: &str,
        book: OrderBook,
        ns: Option<i64>,
        alive: &Alive,
        now_ms: i64,
    ) -> (Option<OrderBook>, Vec<Event>) {
        self.frame_with(role, ticker, book, ns, alive, now_ms, |b| b)
    }

    /// Ревью 427: то же, что frame, но стакан для ядра передаётся в deliver ПОД замком пары (память и канал ядра).
    /// Возвращает: итог deliver (None — не отдавать) и события. Ошибок нет.
    #[allow(clippy::too_many_arguments)]
    pub fn frame_with<R>(
        &self,
        role: Role,
        ticker: &str,
        book: OrderBook,
        ns: Option<i64>,
        alive: &Alive,
        now_ms: i64,
        deliver: impl FnOnce(OrderBook) -> R,
    ) -> (Option<R>, Vec<Event>) {
        let Some(cell) = self.map.get(ticker) else {
            return ((!self.reserve).then(|| deliver(book)), vec![]);
        };
        let mut st = cell.lock().unwrap_or_else(|p| p.into_inner());
        let (give, ev) = self.step(&mut st, role, book, ns, alive, now_ms);
        (give.map(deliver), ev)
    }

    /// Шаг выбора по кадру (под замком пары). Принимает и возвращает — как frame. Ошибок нет.
    fn step(
        &self,
        st: &mut St,
        role: Role,
        book: OrderBook,
        ns: Option<i64>,
        alive: &Alive,
        now_ms: i64,
    ) -> (Option<OrderBook>, Vec<Event>) {
        let r = role as usize;
        let mut ev = vec![];
        st.ok[r] = true;
        if st.lost[r] {
            st.lost[r] = false;
            ev.push(match role {
                Role::Main if st.src == Some(Role::Reserve) => Event::ToMain,
                Role::Main => Event::MainResync,
                Role::Reserve => Event::ReserveResync,
            });
        }
        if !self.reserve {
            st.src = Some(Role::Main);
            return (Some(book), ev);
        }
        let Some(ns) = ns else {
            st.last[r] = None;
            ev.push(Event::NoTime);
            return (None, ev);
        };
        let taken = Taken {
            book,
            alive: alive.clone(),
            seen_ms: now_ms,
        };
        st.last[r] = Some((taken.clone(), ns));
        let src = if st.ok[Role::Main as usize] {
            Role::Main
        } else {
            Role::Reserve
        };
        st.src = Some(src);
        if role != src || st.given_ns.is_some_and(|g| ns < g) {
            return (None, ev);
        }
        st.given_ns = Some(ns);
        (Some(taken.book), ev)
    }

    /// Поток пары потерян. Принимает: поток, тикер. Возвращает: что сделать и события. Ошибок нет; неизвестный тикер —
    /// Invalidate.
    pub fn lost(&self, role: Role, ticker: &str) -> (Out, Vec<Event>) {
        self.lost_with(role, ticker, |o| o)
    }

    /// Ревью 427: то же, что lost, но действие act выполняется ПОД замком пары. Возвращает: итог act и события.
    /// Ошибок нет.
    pub fn lost_with<R>(
        &self,
        role: Role,
        ticker: &str,
        act: impl FnOnce(Out) -> R,
    ) -> (R, Vec<Event>) {
        let Some(cell) = self.map.get(ticker) else {
            return (act(Out::Invalidate), vec![]);
        };
        let mut st = cell.lock().unwrap_or_else(|p| p.into_inner());
        let (o, ev) = self.lose_step(&mut st, role);
        (act(o), ev)
    }

    /// Шаг выбора при потере потока (под замком пары). Принимает: состояние пары, поток. Возвращает — как lost.
    /// Ошибок нет.
    fn lose_step(&self, st: &mut St, role: Role) -> (Out, Vec<Event>) {
        let r = role as usize;
        st.ok[r] = false;
        st.lost[r] = true;
        st.last[r] = None;
        let mut ev = vec![match role {
            Role::Main => Event::MainGap,
            Role::Reserve => Event::ReserveGap,
        }];
        // Служил другой поток — ядру ничего не меняется.
        if st.src.is_some_and(|s| s != role) {
            return (Out::Nothing, ev);
        }
        let o = role.other();
        if !(self.reserve && st.ok[o as usize]) {
            st.src = None;
            if self.reserve {
                ev.push(Event::Blind);
            }
            return (Out::Invalidate, ev);
        }
        st.src = Some(o);
        if role == Role::Main {
            ev.push(Event::ToReserve);
        }
        let given = st.given_ns;
        match st.last[o as usize].clone() {
            Some((t, ns)) if given.is_none_or(|g| ns > g) => {
                st.given_ns = Some(ns);
                (Out::Give(t), ev)
            }
            Some((t, _)) => (Out::Keep(t.alive), ev),
            None => (Out::Invalidate, ev),
        }
    }
}
