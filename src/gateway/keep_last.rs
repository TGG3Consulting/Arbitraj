//! Файл: gateway/keep_last.rs
//! Что делает: решения 321/324 — набор «ждут доставки» для событий, не вошедших в канал ядра (чистая логика над
//! настоящим каналом tokio, без площадки). Перенесён из venues/bsc_dex/pools/pending_events.rs (решение 324) и обобщён:
//! по ключу хранится только ПОСЛЕДНЕЕ (слот сливает новое с ждущим — Slot::merge), порядок — очередь ключей (FIFO,
//! честно для всех классов: кто раньше отложен — раньше доставлен; повторное событие ждущего ключа место в очереди не
//! теряет и не занимает новое); срочное (Slot::urgent — «недостоверно») — в начало очереди и не теряется никогда.
//! - fast (ревью 324, Б1 — быстрый путь): счётчик busy («ключей в наборе») == 0 — без замка, без ключа, без
//!   настройки: try_reserve (Free — вызывающий шлёт этим разрешением; Full; Closed); busy != 0 — Busy.
//! - slow (под замком набора): ключ уже ждёт — слить (Offer::Replaced). Иначе: у класса ключа ждут отложенные
//!   настройкой — за ними без чтения настройки (решение 321: выключение действует, когда класс опустел); очередь не
//!   пуста из-за других классов — `keep` (настройка): да — за ждущими, нет — без ожидания в канал, полон — Offer::Lost;
//!   очередь пуста — без ожидания, полон — `keep`: да — в очередь (Deferred), нет — Lost. Срочное — без ожидания,
//!   полон — в начало очереди без чтения настройки.
//! - offer — fast + slow с готовым ключом (тесты и вызывающие, у которых ключ дешёвый).
//! - deliver (gateway/keep_last_deliver.rs): задача-доставщик, одна на набор; запускает вызывающий по start: true у
//!   Deferred и Replaced (Н3: доставщик упал, а дальше только слияния ждущих ключей).
//!
//! Гарантия порядка быстрого пути (ревью 324, Б1). busy меняется только под замком набора и публикуется (Release) ПОСЛЕ
//! каждого изменения: откладывание — +1 до снятия замка; доставка — −1 только ПОСЛЕ отправки сообщения в канал. Пусть
//! предложение X ключа k началось после возврата предложения Y того же ключа (одна задача по очереди или иначе
//! упорядочены happens-before). Y отправлено сразу — X в канале позже. Y отложено/слито — к возврату Y busy ≥ 1; X
//! читает busy (Acquire): ≥ 1 — медленный путь под замком (порядок — как раньше); 0 — значит, прочитана запись после
//! доставки ВСЕХ ждавших (в том числе слота k), а их отправка happens-before этой записи — X уйдёт в канал позже.
//! Ждущее событие ключа не обгоняется событием, предложенным после его откладывания. Для ПЕРЕКРЫВАЮЩИХСЯ по времени
//! предложений одного ключа из разных задач (факт кода: bitget — поток WS и снимок REST котировки исполнения той же
//! пары; у DEX события пула идут из сессии его пачки — pools/session.rs, последовательно; docs/DECISIONS.md 324 ревью)
//! порядок не задан — как у прежнего try_send: X может прочитать busy = 0, затем Y отложится и будет доставлено, затем X отправится.
//! Каждое событие — полное состояние, ядро держит последнее пришедшее; какое из двух одновременных свежее, не знал и
//! прежний код.
//!
//! Кто вызывает: gateway/core_out.rs (общий набор канала ядра); тесты.
//! От чего зависит: tokio::sync::mpsc, gateway/keep_last_kinds.rs (типы), gateway/keep_last_state.rs (состояние под
//! замком и его шаги), gateway/keep_last_decide.rs (решение медленного пути), gateway/keep_last_deliver.rs (доставщик).
//! Настроек нет (откладывать ли — решает вызывающий через keep).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

pub use super::keep_last_kinds::{Ask, Check, Counts, Fast, Keyed, Offer, Slot, Take};
use super::keep_last_state::State;

/// Набор «ждут доставки». Дешёво клонируется (общий).
pub struct Pending<K: Keyed, S>(Arc<Inner<K, S>>);

/// Состояние под замком и счётчик быстрого пути.
struct Inner<K: Keyed, S> {
    state: Mutex<State<K, S>>,
    /// Ключей в наборе (в очереди и в доставке) — копия State::busy, пишется под замком (см. шапку).
    busy: AtomicUsize,
}

impl<K: Keyed, S> Clone for Pending<K, S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K: Keyed, S> Default for Pending<K, S> {
    fn default() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            busy: AtomicUsize::new(0),
        }))
    }
}

impl<K: Keyed, S: Slot> Pending<K, S> {
    /// Замок состояния (без паники на отравлении). Ошибок нет.
    pub(super) fn lock(&self) -> MutexGuard<'_, State<K, S>> {
        self.0.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Публикует busy (вызывать под замком, после изменения состояния). Ошибок нет.
    pub(super) fn publish(&self, s: &State<K, S>) {
        self.0.busy.store(s.busy(), Ordering::Release);
    }

    /// Быстрый путь (см. шапку): одна атомарная загрузка и try_reserve. Принимает: канал. Возвращает: Fast. Ошибок нет.
    pub fn fast<'a>(&self, out: &'a Sender<S::Msg>) -> Fast<'a, S::Msg> {
        if self.0.busy.load(Ordering::Acquire) != 0 {
            return Fast::Busy;
        }
        match out.try_reserve() {
            Ok(p) => Fast::Free(p),
            Err(TrySendError::Full(())) => Fast::Full,
            Err(TrySendError::Closed(())) => Fast::Closed,
        }
    }

    /// Медленный путь под замком набора (см. шапку). Принимает: канал, ключ, слот, «откладывать ли». Возвращает:
    /// Offer. Ошибок нет.
    pub fn slow(
        &self,
        out: &Sender<S::Msg>,
        key: K,
        item: S,
        keep: &mut Ask<impl FnOnce() -> bool>,
    ) -> Offer {
        let mut s = self.lock();
        let r = s.decide(out, key, item, keep);
        self.publish(&s);
        r
    }

    /// Быстрый путь, иначе медленный — с готовым ключом (см. шапку). Принимает: канал, ключ, слот, «откладывать ли»
    /// (читается не больше раза и только когда без неё не решить). Возвращает: Offer. Ошибок нет.
    pub fn offer(
        &self,
        out: &Sender<S::Msg>,
        key: K,
        item: S,
        keep: impl FnOnce() -> bool,
    ) -> Offer {
        let mut keep = Ask::new(keep);
        match self.fast(out) {
            Fast::Free(p) => {
                p.send(item.now());
                Offer::Sent
            }
            Fast::Closed => Offer::Closed,
            Fast::Full if !item.urgent() && !keep.get() => Offer::Lost,
            Fast::Full | Fast::Busy => self.slow(out, key, item, &mut keep),
        }
    }

    /// Забирает счёт класса с прошлой записи (обнуляет; waiting — сколько ждёт сейчас). Ошибок нет.
    pub fn take(&self, class: &K::Class) -> Counts {
        let mut s = self.lock();
        let c = s.count(class);
        let waiting = c.waiting;
        let taken = std::mem::replace(
            c,
            Counts {
                waiting,
                ..Counts::default()
            },
        );
        Counts { waiting, ..taken }
    }

    /// Счёт класса без обнуления. Ошибок нет.
    pub fn peek(&self, class: &K::Class) -> Counts {
        *self.lock().count(class)
    }

    /// Сколько ключей ждёт сейчас (все классы). Ошибок нет.
    pub fn waiting(&self) -> usize {
        self.lock().order.len()
    }
}
