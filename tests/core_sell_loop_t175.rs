//! Файл: tests/core_sell_loop_t175.rs
//! Что делает: решение 389 (задача t175) — цикл продолжений core/sell_loop.rs на чистой логике (без сети и площадок):
//! (1) цепочка из 100 000 продолжений, каждое откладывает следующее из хвоста (как sell.rs → stuck.rs → … → sell.rs),
//! проходит в потоке с малым стеком SMALL, порядок шагов тот же, что у вложенных вызовов; та же цепочка вложенными
//! вызовами (Box::pin(...).await, как было до решения 389) в таком стеке не помещается — здесь не запускается, она
//! уронила бы процесс; (2) продолжение видит те же признаки задачи, что видело бы вложенным: «ручное»
//! (risk_count::auto / manual поверх цикла) и терпение после 429 (gateway::patience).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::sell_loop, arb::core::risk_count, arb::gateway::patience.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arb::core::risk_count;
use arb::core::sell_loop::{Job, hop};
use arb::gateway::patience;

/// Стек потока теста: заведомо меньше, чем нужно 100 000 вложенным уровням опроса будущих. Параметр теста.
const SMALL: usize = 1024 * 1024;

/// Длина цепочки (параметр теста).
const N: u64 = 100_000;

/// Журнал шагов цепочки: (номер, «до» / «после» откладывания следующего).
type Log = Arc<Mutex<Vec<(u64, &'static str)>>>;

/// Выполняет тело в потоке со стеком SMALL на однопоточном рантайме. Паника тела — паника теста.
fn small<F>(body: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let run = move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(body);
    };
    let t = std::thread::Builder::new().stack_size(SMALL);
    t.spawn(run).unwrap().join().unwrap();
}

/// Шаг n: запись «до», хвостовой переход к шагу n − 1 через hop. Как у продолжений продажи, после hop шаг ничего не
/// делает. Возвращает: продолжение.
fn step(n: u64, log: Log) -> Job {
    Box::pin(async move {
        log.lock().unwrap().push((n, "до"));
        if n > 0 {
            hop(step(n - 1, log)).await;
        }
    })
}

#[test]
fn long_chain_runs_in_small_stack_in_order() {
    small(async {
        let log: Log = Default::default();
        hop(step(N, log.clone())).await;
        let got = log.lock().unwrap().clone();
        let want: Vec<(u64, &str)> = (0..=N).rev().map(|n| (n, "до")).collect();
        assert_eq!(got.len(), want.len());
        assert!(got == want, "порядок шагов — как у вложенных вызовов");
        // После цепочки ячейки цикла нет: новый hop снова сам становится циклом и исполняется сразу.
        let done = Arc::new(Mutex::new(false));
        let d = done.clone();
        hop(Box::pin(async move { *d.lock().unwrap() = true })).await;
        assert!(*done.lock().unwrap());
    });
}

/// Что видит продолжение: (ручное?, есть ли терпение).
type Seen = Arc<Mutex<Vec<(bool, bool)>>>;

/// Последнее звено: записывает признаки задачи. Возвращает: продолжение.
fn look(seen: Seen) -> Job {
    Box::pin(async move {
        let p = patience::deadline_left().is_some();
        seen.lock().unwrap().push((risk_count::is_manual(), p));
    })
}

/// Звено: записывает признаки, затем откладывает `next` внутри `auto` (как sell.rs → risk_count::auto(...)) и под
/// терпением (как market_guarded → patience_scope::with). Возвращает: продолжение.
fn auto_then(seen: Seen, next: Job) -> Job {
    Box::pin(async move {
        let p = patience::deadline_left().is_some();
        seen.lock().unwrap().push((risk_count::is_manual(), p));
        let inner = risk_count::auto(hop(next));
        patience::scope(Duration::from_secs(3600), inner).await;
    })
}

#[test]
fn deferred_job_sees_same_task_marks() {
    small(async {
        let seen: Seen = Default::default();
        // Кнопка человека (ручное) без терпения → звено ставит auto и терпение и откладывает следующее звено, которое
        // откладывает последнее без своих обёрток: оба должны видеть «не ручное» и терпение — как вложенные.
        let last = look(seen.clone());
        let mid: Job = {
            let s = seen.clone();
            Box::pin(async move {
                let p = patience::deadline_left().is_some();
                s.lock().unwrap().push((risk_count::is_manual(), p));
                hop(last).await;
            })
        };
        let first = auto_then(seen.clone(), mid);
        risk_count::manual(hop(first)).await;
        let got = seen.lock().unwrap().clone();
        assert_eq!(got, vec![(true, false), (false, true), (false, true)]);
    });
}
