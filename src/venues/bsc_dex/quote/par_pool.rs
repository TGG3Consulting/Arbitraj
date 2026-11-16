//! Файл: venues/bsc_dex/quote/par_pool.rs
//! Что делает: решение 336 (ревью t157 Н2) — постоянный набор потоков-помощников для quote/par.rs: потоки создаются
//! один раз (и дорастают, когда растёт нужный размер), а не на каждый вызов и не на каждый шаг сплита. Размер —
//! cpu.helper_threads (меняется на лету: лишние потоки выходят, когда освобождаются); не задана — столько, сколько
//! помощников когда-либо просили (dex.quote_threads / dex.graph_threads − 1), только растёт.
//! Как работа с заимствованными данными идёт в чужом потоке: задача вызывающего (замыкание над ссылками на его
//! данные) кладётся в очередь со «щеколдой» (Latch, в куче, у задачи своя ссылка). Помощник начинает задачу, только
//! если щеколда ещё открыта; вызывающий, закончив свою часть, закрывает щеколду и ЖДЁТ все начатые задачи — поэтому
//! заимствованные данные живы всё время, пока их читает помощник (так же устроен std::thread::scope). Задача, взятая
//! из очереди после закрытия, не выполняется — только освобождается память замыкания (ссылки в нём не читаются).
//! Закрытие и ожидание — в Drop стража: и при панике вызывающего. Паника помощника ловится и поднимается у вызывающего.
//! Кто вызывает: quote/par.rs.
//! От чего зависит: std (Mutex, Condvar, thread), ctx.rs (чтение настройки).
//! Настройки: cpu.helper_threads (вся система; необязательная).

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use super::super::ctx::Ctx;

/// Работа помощника (время жизни стёрто — см. шапку и Scope::spawn).
type Job = Box<dyn FnOnce() + Send + 'static>;

/// Паника помощника.
type Panic = Box<dyn Any + Send>;

/// Состояние щеколды: закрыта ли, сколько задач идёт, первая паника помощника.
type LatchState = (bool, usize, Option<Panic>);

/// Щеколда одного вызова.
struct Latch {
    st: Mutex<LatchState>,
    done: Condvar,
}

/// Задача в очереди: щеколда вызова и работа.
type Task = (Arc<Latch>, Job);

/// Очередь задач и размер набора.
struct Pool {
    queue: Mutex<VecDeque<Task>>,
    ready: Condvar,
    alive: AtomicUsize,
    target: AtomicUsize,
}

static POOL: Pool = Pool {
    queue: Mutex::new(VecDeque::new()),
    ready: Condvar::new(),
    alive: AtomicUsize::new(0),
    target: AtomicUsize::new(0),
};

/// Что делает: размер набора для вызова, которому нужно `want` помощников: cpu.helper_threads, если задана; иначе
/// наибольшее из прежнего размера и `want`. Лишние потоки выйдут сами, недостающие создаются здесь (только при росте
/// размера — не на каждом вызове). Принимает: окружение, сколько помощников нужно. Возвращает: размер. Ошибок нет.
pub fn ensure(ctx: &Ctx, want: usize) -> usize {
    let t = match ctx.int("cpu.helper_threads") {
        Ok(n) => usize::try_from(n).unwrap_or(usize::MAX),
        Err(_) => POOL.target.load(Ordering::Acquire).max(want),
    };
    if POOL.target.swap(t, Ordering::AcqRel) > t {
        // Размер уменьшен — лишние потоки проснутся и выйдут.
        POOL.ready.notify_all();
    }
    while POOL.alive.load(Ordering::Acquire) < t {
        POOL.alive.fetch_add(1, Ordering::AcqRel);
        if std::thread::Builder::new()
            .name("bsc-dex-helper".into())
            .spawn(worker)
            .is_err()
        {
            POOL.alive.fetch_sub(1, Ordering::AcqRel);
            break;
        }
    }
    t
}

/// Поток-помощник: берёт задачи по одной; лишний (живых больше размера) выходит, когда свободен. Ошибок нет.
fn worker() {
    loop {
        let (latch, job) = {
            let mut q = POOL.queue.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                let a = POOL.alive.load(Ordering::Acquire);
                if a > POOL.target.load(Ordering::Acquire)
                    && POOL
                        .alive
                        .compare_exchange(a, a - 1, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    return;
                }
                if let Some(t) = q.pop_front() {
                    break t;
                }
                q = POOL.ready.wait(q).unwrap_or_else(|p| p.into_inner());
            }
        };
        let started = {
            let mut g = latch.st.lock().unwrap_or_else(|p| p.into_inner());
            if !g.0 {
                g.1 += 1;
            }
            !g.0
        };
        if !started {
            continue;
        }
        let panic = catch_unwind(AssertUnwindSafe(job)).err();
        let mut g = latch.st.lock().unwrap_or_else(|p| p.into_inner());
        g.1 -= 1;
        if g.2.is_none() {
            g.2 = panic;
        }
        if g.1 == 0 {
            latch.done.notify_all();
        }
    }
}

/// Область одного вызова: задачи, отданные помощникам, живут не дольше её (Drop ждёт начатые).
pub struct Scope<'a> {
    latch: Arc<Latch>,
    _borrow: std::marker::PhantomData<&'a ()>,
}

impl Default for Scope<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Scope<'a> {
    /// Новая область. Ошибок нет.
    pub fn new() -> Scope<'a> {
        Scope {
            latch: Arc::new(Latch {
                st: Mutex::new((false, 0, None)),
                done: Condvar::new(),
            }),
            _borrow: std::marker::PhantomData,
        }
    }

    /// Отдаёт задачу в очередь помощников. Принимает: задачу над данными, живущими не меньше области. Ошибок нет.
    pub fn spawn(&self, f: impl FnOnce() + Send + 'a) {
        let b: Box<dyn FnOnce() + Send + 'a> = Box::new(f);
        // SAFETY: задача начинается только при открытой щеколде, а Drop области закрывает щеколду и ждёт все начатые
        // задачи до того, как заимствованные данные ('a) перестанут жить; не начатая задача только освобождается.
        let job: Job = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + 'a>, Job>(b) };
        let mut q = POOL.queue.lock().unwrap_or_else(|p| p.into_inner());
        q.push_back((self.latch.clone(), job));
        drop(q);
        POOL.ready.notify_one();
    }

    /// Закрывает щеколду и ждёт начатые задачи; паника помощника — здесь же (кроме случая, когда уже идёт своя паника).
    /// Ошибок нет.
    fn close(&self) -> Option<Panic> {
        let mut g = self.latch.st.lock().unwrap_or_else(|p| p.into_inner());
        g.0 = true;
        while g.1 > 0 {
            g = self.latch.done.wait(g).unwrap_or_else(|p| p.into_inner());
        }
        g.2.take()
    }

    /// Закрыть область явно (после своей части работы) и поднять панику помощника. Ошибок нет.
    pub fn finish(self) {
        if let Some(p) = self.close() {
            resume_unwind(p);
        }
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        // Повторное закрытие безопасно: щеколда уже закрыта, ждать некого.
        let _ = self.close();
    }
}
