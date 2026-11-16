//! Файл: venues/bsc_dex/node/prefetch.rs
//! Что делает: решение 325 — «заранее прочитанные ответы» одной работы (перечитывание пула по событию): ответы
//! view-вызовов на ОДНОМ блоке, прочитанные заранее одним (или несколькими) eth_call к Multicall3.aggregate3, кладутся
//! в книгу на задаче tokio (task_local, как node/req_trail.rs). Пока книга стоит на задаче, node/call.rs view и
//! node/multicall.rs aggregate сначала смотрят в неё: ответ есть (тот же контракт, те же данные вызова, тот же блок) —
//! узел не спрашивается; нет — вызов идёт к узлу как раньше. Книга — только подсказка: код чтения пула тот же, разбор
//! тот же, ошибка вызова (откат, ошибка узла) в книгу не попадает — такой вызов уходит к узлу и даёт ту же ошибку, что
//! без книги. Книга помнит, какие вызовы сделал код чтения (на её блоке), — для следующего перечитывания этого пула
//! (pools/prefetch_memo.rs).
//! Кто вызывает: node/call.rs (view), node/multicall.rs (aggregate: active, затем hit), pools/solidly_fee_read.rs
//! (fee) — hit; pools/prefetch_read.rs — Book, scope.
//! От чего зависит: tokio (task_local).
//! Настроек нет (включает вызывающий — dex.event_refresh_batch, решение 325).
//!
//! Равенство ответа: returnData успешного вызова внутри Multicall3.aggregate3 — те же байты, что вернул бы отдельный
//! eth_call view-функции на том же блоке (https://github.com/mds1/multicall3/blob/main/src/Multicall3.sol#L98-L119:
//! target.call(callData), результат — (success, returnData) без изменений). Так уже читают node/call.rs view_many
//! (решение 83) и pools/load_v3_once.rs (решение 315).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

use alloy_primitives::{Address, Bytes};

/// Вызов: контракт и данные (ABI-кодированный вызов функции).
pub type Call = (Address, Bytes);

/// Книга работы: блок, прочитанные ответы, что уже спрашивали, какие вызовы сделал код чтения. Общая (Arc).
#[derive(Debug, Clone, Default)]
pub struct Book(Arc<Mutex<Inner>>);

#[derive(Debug, Default)]
struct Inner {
    /// Блок-тег, на котором прочитаны ответы (тот же, что у кода чтения: node/call.rs block_tag).
    tag: String,
    /// Успешные ответы.
    have: HashMap<Call, Bytes>,
    /// Вызовы, уже отправленные заранее (успешно или нет) — второй раз заранее не спрашиваются.
    asked: HashSet<Call>,
    /// Вызовы кода чтения на блоке книги, по порядку, без повторов.
    used: Vec<Call>,
    seen: HashSet<Call>,
    /// Из книги / мимо книги (к узлу).
    hits: usize,
    misses: usize,
}

impl Book {
    /// Пустая книга на блоке `tag`. Ошибок нет.
    pub fn new(tag: &str) -> Book {
        let b = Book::default();
        b.lock().tag = tag.to_string();
        b
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Из списка — только вызовы, которых ещё не спрашивали (без повторов, порядок прежний). Ошибок нет.
    pub fn fresh(&self, calls: Vec<Call>) -> Vec<Call> {
        let g = self.lock();
        let mut seen = HashSet::new();
        calls
            .into_iter()
            .filter(|c| !g.asked.contains(c) && seen.insert(c.clone()))
            .collect()
    }

    /// Кладёт ответы заранее прочитанных вызовов (по порядку): успешный — в книгу; откат или сбой части — нет.
    /// Принимает: вызовы, ответы (Ok — байты, Err — причина). Ошибок нет.
    pub fn fill(&self, calls: &[Call], answers: &[Result<Bytes, String>]) {
        let mut g = self.lock();
        for (i, c) in calls.iter().enumerate() {
            g.asked.insert(c.clone());
            if let Some(Ok(b)) = answers.get(i) {
                g.have.insert(c.clone(), b.clone());
            }
        }
    }

    /// Ответ вызова из книги (без учёта «кто спросил»). Ошибок нет.
    pub fn get(&self, to: Address, data: &Bytes) -> Option<Bytes> {
        self.lock().have.get(&(to, data.clone())).cloned()
    }

    /// Вызовы, сделанные кодом чтения на блоке книги (для следующего перечитывания пула). Ошибок нет.
    pub fn used(&self) -> Vec<Call> {
        self.lock().used.clone()
    }

    /// (из книги, к узлу) — сколько вызовов кода чтения ответила книга и сколько ушло к узлу. Ошибок нет.
    pub fn counts(&self) -> (usize, usize) {
        let g = self.lock();
        (g.hits, g.misses)
    }

    /// Вызов кода чтения: на блоке книги — отмечается; ответ — из книги, если есть. Ошибок нет.
    fn look(&self, to: Address, data: &Bytes, tag: &str) -> Option<Bytes> {
        let mut g = self.lock();
        if g.tag != tag {
            return None;
        }
        let key = (to, data.clone());
        if g.seen.insert(key.clone()) {
            g.used.push(key.clone());
        }
        let got = g.have.get(&key).cloned();
        match got {
            Some(_) => g.hits += 1,
            None => g.misses += 1,
        }
        got
    }
}

tokio::task_local! {
    static BOOK: Book;
}

/// Выполняет работу с книгой на задаче. Принимает: книгу, работу. Возвращает: её результат. Ошибок нет.
pub async fn scope<F: Future>(book: Book, work: F) -> F::Output {
    BOOK.scope(book, work).await
}

/// Ответ вызова из книги задачи. Книги нет, другой блок или ответа нет — None (вызов идёт к узлу, как раньше).
/// Принимает: контракт, данные вызова, блок-тег запроса. Ошибок нет.
pub fn hit(to: Address, data: &Bytes, tag: &str) -> Option<Bytes> {
    BOOK.try_with(|b| b.look(to, data, tag)).ok().flatten()
}

/// Ревью 325 (Н1): стоит ли книга на задаче — одна проверка task_local, без поиска и выделений (node/multicall.rs
/// aggregate: книги нет — сразу к узлу, как до решения 325). Ошибок нет.
pub fn active() -> bool {
    BOOK.try_with(|_| ()).is_ok()
}
