//! Файл: core/busy_calc.rs
//! Что делает: решение 267 (Тигран 13.11, включаемое) — замер «что было выгодно, пока монета занята сделкой». Пока по
//! монете действует замок «одна сделка на монету» (решение 118, Я-2), очередь кандидатов её направления не считает
//! (candidates.rs enqueue молча выходит) — бот не знает, выросла ли разница цен за время сделки. При window.busy_calc =
//! да тик вместо молчаливого выхода ставит ТЕНЕВОЙ расчёт направления (не чаще одного за window.busy_calc_min_ms на
//! направление, не больше одного одновременно; ПЕРВЫЙ — не раньше чем через тот же интервал после того, как тик
//! впервые принёс направление монеты при занятом замке: не идти одновременно с предпроверкой и ордером нашей сделки и
//! не делить с ними счёт котировок DEX): тот же profit::decide с теми же настройками, что у кандидата; бюджет — по тем
//! же лимитам, но БЕЗ вычета текущей сделки этой монеты (busy_budget.rs: «если бы мы не были заняты»; свободный баланс
//! не читается, как у кандидата); итог идёт ТОЛЬКО в сводку замера (busy_note.rs → окно монеты и журнал core.busy_calc).
//! Теневой расчёт НЕ вызывает исполнителя, не открывает и не продлевает окно как «прошедший кандидат», не трогает
//! эпизоды, строку и карточку «Кандидатов», журнал отказов кандидатов, отказов исполнителя и расчёт «без потолка».
//! Торговля не меняется: только наблюдение.
//! Место расчёта: своя очередь (не канал кандидатов — теневые не занимают в нём места), её разбирает та же задача
//! candidates.rs run и только когда очередь настоящих кандидатов пуста; предел profit.calc_concurrency общий — теневой
//! стартует, только если настоящих и теневых вместе меньше предела, иначе отбрасывается (не ждёт); условие допуска
//! настоящего кандидата прежнее (теневые в его счётчик не входят), а если настоящий занял место и вместе стало больше
//! предела — последний теневой снимается (отмена decide безопасна, как у срока расчёта). Срок — profit.calc_timeout_ms.
//! Класс срочности запросов — Background (как у расчёта «без потолка», решение 141), не Trade.
//! Не задана или «нет» window.busy_calc — одно чтение этой настройки и выход: ни расчёта, ни памяти, ни записи.
//! Задана «да», а window.busy_calc_min_ms не задана — замер не работает, одна запись в журнал на смену причины.
//! Кто вызывает: core/candidates.rs (enqueue — монета занята; run — Shadows), core/inner.rs (State).
//! От чего зависит: profit.rs (decide), dry.rs, limits_parts.rs, busy_budget.rs, busy_sum.rs, busy_note.rs,
//! gateway/urgency.rs, settings.
//! Настройки: window.busy_calc, window.busy_calc_min_ms; profit.calc_concurrency, profit.calc_timeout_ms (те же, что у
//! очереди кандидатов).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::timeout;

use crate::core::types::Direction;
use crate::gateway::urgency::{self, Urgency};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::busy_sum::{BusyBest, BusySum, Outcome};
use super::coin_key::CoinKey;
use super::costs::Calc;
use super::dry::Mode;
use super::engine::{Core, lk};
use super::profit::{self, Verdict};

/// Включатель замера (да/нет, вся система).
pub const ON: &str = "window.busy_calc";
/// Не чаще одного теневого расчёта на направление за столько мс.
pub const MIN_MS: &str = "window.busy_calc_min_ms";
/// Вид записи журнала.
pub const KIND: &str = "core.busy_calc";

/// Отметка направления: когда поставлен прошлый теневой расчёт и идёт ли он (ждёт в очереди или считается).
struct Mark {
    last: Instant,
    running: bool,
}

/// Занятость монеты, как её видит замер: когда тик впервые принёс её направление при занятом замке, и её сделка.
struct Seen {
    since: Instant,
    /// id открытой сделки монеты из учёта (None — ещё не видна: замок держит предпроверка).
    deal: Option<String>,
}

/// Память замера (Inner.busy_calc).
pub struct State {
    tx: mpsc::Sender<Direction>,
    /// Приёмник очереди теневых расчётов — забирает задача очереди кандидатов (candidates.rs run).
    rx: Mutex<Option<mpsc::Receiver<Direction>>>,
    marks: Mutex<HashMap<Direction, Mark>>,
    /// С какого момента монета занята (первое обращение тика при занятом замке) — первый теневой расчёт не раньше
    /// чем через интервал после него.
    seen: Mutex<HashMap<CoinKey, Seen>>,
    /// Сводки направлений занятых монет (busy_note.rs).
    pub(super) sums: Mutex<HashMap<Direction, BusySum>>,
    /// Монеты, за концом занятости которых следит задача (busy_note.rs watch).
    pub(super) watching: Mutex<HashSet<CoinKey>>,
    /// Почему замер включён, но не работает (уже записано в журнал); None — работает или не проверялось.
    off: Mutex<Option<String>>,
}

impl State {
    /// Пустая память. Принимает: размер очереди (тот же --live-buffer, что у остальных каналов ядра). Ошибок нет.
    pub fn new(buffer: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer);
        State {
            tx,
            rx: Mutex::new(Some(rx)),
            marks: Mutex::default(),
            seen: Mutex::default(),
            sums: Mutex::default(),
            watching: Mutex::default(),
            off: Mutex::default(),
        }
    }

    /// Забывает начало занятости монеты и отметки её направлений, расчёт которых не идёт (монета освободилась).
    /// Ошибок нет.
    pub(super) fn forget(&self, key: &CoinKey) {
        lk(&self.seen).remove(key);
        lk(&self.marks).retain(|d, m| m.running || &CoinKey::of(&d.asset) != key);
    }
}

/// window.busy_calc_min_ms. Ошибки: текст — не задана или 0.
pub(super) fn min_ms(core: &Core) -> Result<u64, String> {
    match core.inner.deps.settings.integer(MIN_MS, At::default()) {
        Ok(0) => Err(format!("{MIN_MS} = 0")),
        Ok(ms) => Ok(ms),
        Err(e) => Err(e.message),
    }
}

/// Что делает: монета занята сделкой, а тик ставит её направление в расчёт — при включённом замере ставит теневой
/// расчёт (не чаще интервала, не больше одного на направление). Только память и try_send — тик не ждёт.
/// Принимает: ядро, направление. Ошибок нет: выключено — ничего; интервал не задан — одна запись в журнал.
pub fn enqueue(core: &Core, d: &Direction) {
    let s = &core.inner.deps.settings;
    if !s.flag(ON, At::default()).unwrap_or(false) {
        return;
    }
    let st = &core.inner.busy_calc;
    let min = match min_ms(core) {
        Ok(ms) => Duration::from_millis(ms),
        Err(why) => return unset(core, why),
    };
    lk(&st.off).take();
    let now = Instant::now();
    let key = CoinKey::of(&d.asset);
    {
        // Первое обращение по занятой монете — только запомнить момент: первый теневой расчёт не раньше чем через
        // интервал (не идти одновременно с предпроверкой и ордером нашей сделки). Конец занятости ловит задача монеты.
        let mut seen = lk(&st.seen);
        let Some(x) = seen.get(&key) else {
            let since = Seen {
                since: now,
                deal: None,
            };
            seen.insert(key.clone(), since);
            drop(seen);
            return super::busy_note::watch_coin(core, key);
        };
        if now.duration_since(x.since) < min {
            return;
        }
    }
    if lk(&st.marks)
        .get(d)
        .is_some_and(|m| m.running || now.duration_since(m.last) < min)
    {
        return;
    }
    // Сделка монеты сменилась, а конец прежней занятости задача ещё не увидела — занятость считается заново.
    let deal = super::busy_note::open_deal(core, &key).map(|x| x.id);
    {
        let mut seen = lk(&st.seen);
        let Some(x) = seen.get_mut(&key) else { return };
        if x.deal.is_some() && deal.is_some() && x.deal != deal {
            x.since = now;
            x.deal = deal;
            return;
        }
        if deal.is_some() {
            x.deal = deal;
        }
    }
    {
        let mut marks = lk(&st.marks);
        if marks
            .get(d)
            .is_some_and(|m| m.running || now.duration_since(m.last) < min)
        {
            return;
        }
        let m = Mark {
            last: now,
            running: true,
        };
        marks.insert(d.clone(), m);
    }
    if st.tx.try_send(d.clone()).is_err() {
        // Очередь теневых полна — как у кандидата: следующий тик поставит снова.
        lk(&st.marks).remove(d);
    }
}

/// Замер включён, но интервал не задан — одна запись в журнал на смену причины; торговля не затронута. Ошибок нет.
fn unset(core: &Core, why: String) {
    let fresh = {
        let mut off = lk(&core.inner.busy_calc.off);
        let changed = off.as_deref() != Some(why.as_str());
        *off = Some(why.clone());
        changed
    };
    if !fresh {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let text = format!(
            "замер «пока монета занята сделкой» ({ON} = да) не работает: {why}; торговля не затронута"
        );
        let deps = &core.inner.deps;
        deps.log(Level::Warn, KIND, None, text, None).await;
    });
}

/// Теневые расчёты в задаче очереди кандидатов (candidates.rs run): приёмник и идущие расчёты.
pub struct Shadows {
    rx: Option<mpsc::Receiver<Direction>>,
    running: Vec<AbortHandle>,
}

impl Shadows {
    /// Забирает приёмник очереди теневых (один раз, при старте задачи очереди). Ошибок нет.
    pub fn take(core: &Core) -> Self {
        Shadows {
            rx: lk(&core.inner.busy_calc.rx).take(),
            running: Vec::new(),
        }
    }

    /// Следующее направление очереди теневых; очереди нет — не возвращается никогда. Безопасно для select. Ошибок нет.
    pub async fn recv(&mut self) -> Direction {
        if let Some(rx) = &mut self.rx
            && let Some(d) = rx.recv().await
        {
            return d;
        }
        std::future::pending().await
    }

    /// Что делает: запускает теневой расчёт, если настоящих и теневых вместе меньше предела profit.calc_concurrency
    /// (0 / не задана — без предела); иначе отбрасывает (в сводку — «отброшено»). Принимает: ядро, направление,
    /// сколько сейчас считается настоящих кандидатов. Ошибок нет.
    pub fn start(&mut self, core: &Core, d: Direction, real: usize) {
        self.running.retain(|h| !h.is_finished());
        let mut job = Job {
            core: core.clone(),
            d,
            done: false,
        };
        if cap(core).is_some_and(|c| real + self.running.len() >= c) {
            // Мест нет — не ждём и не вытесняем: Drop снимает отметку и пишет «отброшено».
            return;
        }
        let h = tokio::spawn(async move {
            let out = one(&job.core, &job.d).await;
            job.done = true;
            super::busy_note::record(&job.core, &job.d, out);
        });
        self.running.push(h.abort_handle());
    }

    /// Что делает: настоящий кандидат занял место — если вместе с теневыми стало больше предела, последние теневые
    /// снимаются (уступают). Теневых нет — ничего (ни одной блокировки). Принимает: предел, сколько настоящих.
    pub fn give_way(&mut self, cap: Option<usize>, real: usize) {
        if self.running.is_empty() {
            return;
        }
        self.running.retain(|h| !h.is_finished());
        let Some(c) = cap else { return };
        while real + self.running.len() > c {
            match self.running.pop() {
                Some(h) => h.abort(),
                None => break,
            }
        }
    }
}

/// Теневой расчёт в работе: при любом конце (итог, отброшен, снят) снимает отметку «идёт»; без итога — «отброшено».
struct Job {
    core: Core,
    d: Direction,
    done: bool,
}

impl Drop for Job {
    fn drop(&mut self) {
        if let Some(m) = lk(&self.core.inner.busy_calc.marks).get_mut(&self.d) {
            m.running = false;
        }
        if !self.done {
            super::busy_note::record(&self.core, &self.d, Outcome::Dropped);
        }
    }
}

/// profit.calc_concurrency — тот же предел, что читает очередь кандидатов (0 или не задана — без предела). Ошибок нет.
fn cap(core: &Core) -> Option<usize> {
    let s = &core.inner.deps.settings;
    s.integer("profit.calc_concurrency", At::default())
        .ok()
        .filter(|n| *n > 0)
        .map(|n| n as usize)
}

/// Что делает: один теневой расчёт — profit::decide в режиме, прочитанном на расчёт (как у кандидата), под сроком
/// profit.calc_timeout_ms, классом Background. Больше ничего не вызывает. Принимает: ядро, направление.
/// Возвращает: итог для сводки. Ошибок нет — отказ и срок идут в Outcome::Reject.
async fn one(core: &Core, d: &Direction) -> Outcome {
    let s = &core.inner.deps.settings;
    let mode = super::dry::mode(s);
    // Бюджет — без вычета текущей сделки монеты (busy_budget.rs: признак задачи, действует только здесь).
    let calc = async {
        match profit::decide(core, d, &mode).await {
            Verdict::Accept(c) => Outcome::Accept(Box::new(best(core, &c, &mode))),
            Verdict::Reject { reason, .. } => Outcome::Reject(reason),
        }
    };
    let key = CoinKey::of(&d.asset);
    let work = super::busy_budget::scope(key, urgency::scope(Urgency::Background, calc));
    let limit = s.integer("profit.calc_timeout_ms", At::default());
    match limit.ok().filter(|ms| *ms > 0) {
        Some(ms) => match timeout(Duration::from_millis(ms), work).await {
            Ok(v) => v,
            Err(_) => {
                let why = format!("расчёт не уложился в срок profit.calc_timeout_ms {ms} мс");
                Outcome::Reject(why)
            }
        },
        None => work.await,
    }
}

/// Числа прошедшего расчёта для сводки; составляющие лимита (уже без вычета текущей сделки монеты) и сама её доля
/// читаются сразу после расчёта (учёт ядра, без запросов); вызывается внутри признака теневого расчёта.
fn best(core: &Core, c: &Calc, mode: &Mode) -> BusyBest {
    BusyBest {
        profit_usd: c.profit_usd,
        invested: c.invested,
        qty: c.qty,
        at: c.at,
        budget: c.size.as_ref().map(|s| s.budget),
        limits: super::limits_parts::parts(core, &c.direction, mode).ok(),
        freed: super::busy_budget::freed(core, &c.direction).map(|f| f.left),
    }
}
