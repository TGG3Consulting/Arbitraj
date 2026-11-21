//! Файл: core/calc_timing.rs
//! Что делает: замер этапов расчёта кандидата (решение 129). На задачу расчёта ставится общий замер (task_local, как
//! класс срочности); profit.rs и costs.rs оборачивают свои шаги в `stage` — этап верхнего уровня (контекст, бюджет,
//! подбор объёма, финальная котировка) и его части (котировка стороны A/B у площадки, стоимость вывода, курсы затрат).
//! Части одного имени складываются (подбор объёма считает десятки объёмов): сумма времени, сколько раз, запросы к
//! узлу BSC и агрегатору (gateway/calc_meter.rs: вызовов, ожидание очереди, время запроса). Незакрытый этап — тот, на
//! котором расчёт стоит сейчас: по нему причина отказа по сроку называет этап (calc_report.rs). Без замера на задаче
//! `stage` просто выполняет работу. Решение 265: пересчёт сделки при log.step_timing = да тоже идёт под замером —
//! подробным (Timing::detailed, `detail`): только в нём пишутся части calc_detail.rs; замер кандидата — как до 265.
//! Кто вызывает: core/candidates.rs (scope, отчёт), core/step_timing.rs (scope, Timing::detailed, отчёт),
//! core/profit.rs и core/costs.rs (stage, stage_sync), core/calc_detail.rs (detail, stage, stage_sync — части
//! profit.rs, size_search.rs, size_memo.rs), core/calc_report.rs (снимок).
//! От чего зависит: gateway/calc_meter.rs, tokio (task_local).
//! Настройки: нет — замер всегда (на шаг: два чтения часов и одна короткая блокировка; дёшево против котировок).

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::gateway::calc_meter::{Meter, Snap};

/// Сумма по этапу или его части.
#[derive(Debug, Clone, Default)]
pub struct Agg {
    pub name: String,
    pub us: u64,
    pub count: u32,
    pub node: Snap,
    pub agg: Snap,
    /// Части этапа (только у этапа верхнего уровня).
    pub parts: Vec<Agg>,
}

/// Открытый (идущий) этап.
#[derive(Debug, Clone)]
pub struct Open {
    pub id: u64,
    pub name: String,
    /// Имя этапа верхнего уровня, в котором идёт эта часть; None — сам этап верхнего уровня.
    pub parent: Option<String>,
    pub at: Instant,
    pub node0: Snap,
    pub agg0: Snap,
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    done: Vec<Agg>,
    open: Vec<Open>,
}

/// Замер одного расчёта: начало (постановка в очередь), старт расчёта, счётчик запросов, этапы.
#[derive(Debug)]
pub struct Timing {
    pub queued_at: Instant,
    pub started: Instant,
    pub meter: Arc<Meter>,
    /// Решение 265: подробный замер (сделка при log.step_timing = да) — пишутся и части `stage_detail`.
    pub detail: bool,
    state: Mutex<State>,
}

/// Снимок замера для отчёта: закрытые этапы (с частями) и открытые.
pub struct View {
    pub done: Vec<Agg>,
    pub open: Vec<Open>,
}

tokio::task_local! {
    static TIMING: Arc<Timing>;
}

impl Timing {
    /// Новый замер. Принимает: когда направление встало в очередь. Старт — сейчас. Ошибок нет.
    pub fn new(queued_at: Instant) -> Arc<Timing> {
        Arc::new(Timing {
            queued_at,
            started: Instant::now(),
            meter: Arc::new(Meter::default()),
            detail: false,
            state: Mutex::new(State::default()),
        })
    }

    /// Решение 265: подробный замер (пересчёт сделки, core/step_timing.rs). Ошибок нет.
    pub fn detailed(queued_at: Instant) -> Arc<Timing> {
        let mut t = Timing::new(queued_at);
        if let Some(x) = Arc::get_mut(&mut t) {
            x.detail = true;
        }
        t
    }

    /// Снимок этапов. Ошибок нет.
    pub fn view(&self) -> View {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        View {
            done: s.done.clone(),
            open: s.open.clone(),
        }
    }

    /// Открывает этап. Принимает: имя. Возвращает: номер этапа. Ошибок нет.
    fn open(&self, name: String) -> u64 {
        let (node0, agg0) = self.meter.snapshot();
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.next += 1;
        let id = s.next;
        let parent = s.open.first().map(|o| o.name.clone());
        s.open.push(Open {
            id,
            name,
            parent,
            at: Instant::now(),
            node0,
            agg0,
        });
        id
    }

    /// Закрывает этап: время и запросы — в сумму этапа (или части этапа верхнего уровня). Ошибок нет.
    fn close(&self, id: u64) {
        let (node1, agg1) = self.meter.snapshot();
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(i) = s.open.iter().position(|o| o.id == id) else {
            return;
        };
        let o = s.open.remove(i);
        let add = |a: &mut Agg| {
            a.us += o.at.elapsed().as_micros() as u64;
            a.count += 1;
            a.node = sum(a.node, node1.since(o.node0));
            a.agg = sum(a.agg, agg1.since(o.agg0));
        };
        let list = match &o.parent {
            None => &mut s.done,
            Some(p) => match s.done.iter().position(|a| &a.name == p) {
                Some(j) => &mut s.done[j].parts,
                None => {
                    // Родитель ещё идёт: заводим его строку (время родителя добавится при его закрытии).
                    s.done.push(Agg {
                        name: p.clone(),
                        ..Agg::default()
                    });
                    let j = s.done.len() - 1;
                    &mut s.done[j].parts
                }
            },
        };
        match list.iter_mut().find(|a| a.name == o.name) {
            Some(a) => add(a),
            None => {
                let mut a = Agg {
                    name: o.name.clone(),
                    ..Agg::default()
                };
                add(&mut a);
                list.push(a);
            }
        }
    }
}

/// Сумма снимков двух этапов. Ошибок нет.
fn sum(a: Snap, b: Snap) -> Snap {
    Snap {
        calls: a.calls + b.calls,
        wait_us: a.wait_us + b.wait_us,
        send_us: a.send_us + b.send_us,
    }
}

/// Выполняет расчёт с замером. Принимает: замер, работу. Возвращает: её результат. Ошибок нет.
pub async fn scope<F: Future>(t: Arc<Timing>, work: F) -> F::Output {
    TIMING.scope(t, work).await
}

/// Этап расчёта: время и запросы работы — в замер (если он есть на задаче). Принимает: имя, работу.
/// Возвращает: её результат. Работа отменена (срок расчёта) — этап остаётся открытым: на нём расчёт и стоял.
pub async fn stage<F: Future>(name: impl FnOnce() -> String, work: F) -> F::Output {
    let Ok(t) = TIMING.try_with(Arc::clone) else {
        return work.await;
    };
    let id = t.open(name());
    let out = work.await;
    t.close(id);
    out
}

/// То же для синхронного шага (контекст направления). Принимает: имя, шаг. Возвращает: его результат.
pub fn stage_sync<T>(name: impl FnOnce() -> String, work: impl FnOnce() -> T) -> T {
    let Ok(t) = TIMING.try_with(Arc::clone) else {
        return work();
    };
    let id = t.open(name());
    let out = work();
    t.close(id);
    out
}

/// Решение 265: подробный ли замер на задаче (Timing::detailed); замера нет или он обычный — нет. Ошибок нет.
pub fn detail() -> bool {
    TIMING.try_with(|t| t.detail).unwrap_or(false)
}
