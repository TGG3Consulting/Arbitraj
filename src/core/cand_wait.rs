//! Файл: core/cand_wait.rs
//! Что делает: решение 334 (задача t155) — кандидат не пропадает молча. Раньше кандидат выбрасывался в двух местах:
//! очередь расчётов (канал cand_tx, ёмкость --live-buffer) полна — enqueue; достигнут предел одновременных расчётов
//! profit.calc_concurrency — run. Тихая монета (тиков мало) теряла кандидата до следующего тика, и это нигде не было
//! видно.
//! - Всегда (минимум): каждая такая потеря — счёт и запись журнала core.candidate_lost (gateway/lost_log.rs: на 1-ю,
//!   2-ю, 4-ю… потерю или не чаще log.events_lost_interval_s).
//! - profit.calc_wait_ms задана: кандидат не выбрасывается, а ЖДЁТ места в памяти ядра не дольше этого срока (с момента,
//!   когда места не нашлось). Место освободилось — ждущие стартуют первыми, по порядку (FIFO), раньше новых из канала
//!   и раньше теневых расчётов (решение 267). Срок вышел — потеря (счёт и журнал), направление снимается из очереди:
//!   следующий тик поставит его снова. Не задана — как раньше: выброшен (теперь со счётом).
//!
//! Шквал: ждущих не больше числа направлений (направление стоит в очереди один раз — Inner.queued, повторный тик того же
//! направления с ним сливается); каждый ждёт не дольше profit.calc_wait_ms; расчёт — тот же (класс Trade, срок
//! profit.calc_timeout_ms считается от СТАРТА расчёта, ожидание в него не входит и видно в замере «очередь до старта»,
//! решение 129). Расчёт берёт рынок на момент старта, а не на момент тика.
//! Ревью 333: Н5 — срок ожидания через checked_add (огромная настройка — ждёт без срока, а не паника); Н6 — смена
//! profit.calc_concurrency будит задачу очереди (cap_changed): поднятый предел сразу пускает ждущих.
//! Кто вызывает: core/candidates.rs (enqueue — канал полон; run — предел, старт ждущих, срок, смена предела).
//! От чего зависит: engine.rs, gateway/lost_log.rs, settings (live — рассылка изменений), tokio (Notify, broadcast).
//! Настройки: profit.calc_wait_ms (вся система; необязательная); журнал — log.events_lost_interval_s.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::sync::broadcast::{Receiver, error::RecvError};

use crate::core::types::Direction;
use crate::gateway::lost_log::{Lost, Note};
use crate::settings::live::Change;
use crate::settings::read::At;

use super::engine::{Core, lk};

/// Предел одновременных расчётов (core/candidates.rs).
pub const CAP: &str = "profit.calc_concurrency";
/// Сколько кандидат ждёт места, мс (решение 334).
pub const WAIT_MS: &str = "profit.calc_wait_ms";
/// Вид записи журнала о потерях кандидатов.
pub const LOST_KIND: &str = "core.candidate_lost";

/// Почему кандидат потерян.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Очередь расчётов (канал) полна.
    Full,
    /// Достигнут предел profit.calc_concurrency.
    Cap,
    /// Ждал места дольше profit.calc_wait_ms.
    Expired,
}

/// Ждущий места кандидат.
struct Parked {
    d: Direction,
    /// Когда поставлен в очередь (замер «очередь до старта», решение 129).
    at: Instant,
    /// До какого момента ждёт; None — срок не представим (огромная profit.calc_wait_ms — ревью 333, Н5): ждёт места
    /// без срока, а не паника.
    until: Option<Instant>,
}

/// Ждущие места кандидаты и счёт потерь (Inner.cand_wait).
#[derive(Default)]
pub struct Waiting {
    q: Mutex<VecDeque<Parked>>,
    /// Толчок задаче очереди: место освободилось или появился ждущий.
    pub free: Notify,
    lost_full: Lost,
    lost_cap: Lost,
    lost_expired: Lost,
}

/// profit.calc_wait_ms: None — не задана или 0 (ждать не велено). Ошибок нет.
pub fn wait_for(core: &Core) -> Option<Duration> {
    core.inner
        .deps
        .settings
        .integer(WAIT_MS, At::default())
        .ok()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
}

impl Waiting {
    /// Ставит кандидата ждать места. Принимает: направление, момент постановки в очередь, срок ожидания. Ошибок нет.
    pub fn park(&self, d: Direction, at: Instant, wait: Duration) {
        // Ревью 333, Н5: checked_add — огромная настройка не роняет задачу очереди паникой переполнения.
        let until = Instant::now().checked_add(wait);
        lk(&self.q).push_back(Parked { d, at, until });
        self.free.notify_one();
    }

    /// Есть ли ждущие. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        lk(&self.q).is_empty()
    }

    /// Ближайший срок ждущих (None — ждущих нет). Ошибок нет.
    pub fn next_due(&self) -> Option<Instant> {
        lk(&self.q).iter().filter_map(|p| p.until).min()
    }

    /// Первый ждущий, чей срок не вышел (вышедшие по пути — потеря). Вызывать, только когда место есть.
    /// Принимает: ядро. Возвращает: (направление, момент постановки) или None. Ошибок нет.
    pub fn pop(&self, core: &Core) -> Option<(Direction, Instant)> {
        let now = Instant::now();
        loop {
            let p = lk(&self.q).pop_front()?;
            if p.until.is_none_or(|u| u > now) {
                return Some((p.d, p.at));
            }
            lost(core, &p.d, Why::Expired);
        }
    }

    /// Снимает ждущих с вышедшим сроком (потеря — счёт и журнал, направление снимается из очереди). Ошибок нет.
    pub fn expire(&self, core: &Core) {
        let now = Instant::now();
        let gone: Vec<Direction> = {
            let mut q = lk(&self.q);
            let (old, keep): (Vec<Parked>, Vec<Parked>) =
                q.drain(..).partition(|p| p.until.is_some_and(|u| u <= now));
            q.extend(keep);
            old.into_iter().map(|p| p.d).collect()
        };
        for d in gone {
            lost(core, &d, Why::Expired);
        }
    }
}

/// Кандидат потерян: снимается из очереди (следующий тик поставит снова), счёт и запись журнала (gateway/lost_log.rs).
/// Принимает: ядро, направление, причину. Ошибок нет.
pub fn lost(core: &Core, d: &Direction, why: Why) {
    lk(&core.inner.queued).remove(d);
    let w = &core.inner.cand_wait;
    let (counter, what) = match why {
        Why::Full => (
            &w.lost_full,
            "кандидат не посчитан: очередь расчётов полна (profit.calc_wait_ms не задана — не ждёт)",
        ),
        Why::Cap => (
            &w.lost_cap,
            "кандидат не посчитан: достигнут предел profit.calc_concurrency (profit.calc_wait_ms не задана — не ждёт)",
        ),
        Why::Expired => (
            &w.lost_expired,
            "кандидат не посчитан: ждал места дольше profit.calc_wait_ms",
        ),
    };
    let note = Note {
        kind: LOST_KIND,
        venue: Some(&d.buy_on.0),
        what,
        last: format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0),
    };
    let deps = &core.inner.deps;
    counter.hit(&deps.settings, &deps.journal, note);
}

/// Ревью 333, Н6: ждёт смены profit.calc_concurrency (поднятый предел — место для ждущих появилось без конца расчёта).
/// Принимает: подписку на изменения настроек (None — рассылка закрыта: ждёт вечно, холостого цикла нет). Возвращает:
/// когда предел сменился (или подписка отстала — смена могла пропасть: лишняя проверка безвредна). Ошибок нет.
pub async fn cap_changed(rx: &mut Option<Receiver<Change>>) {
    loop {
        let Some(r) = rx.as_mut() else {
            return std::future::pending().await;
        };
        match r.recv().await {
            Ok(c) if c.name == CAP => return,
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => return,
            Err(RecvError::Closed) => *rx = None,
        }
    }
}
