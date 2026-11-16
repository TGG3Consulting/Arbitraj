//! Файл: venues/bsc_dex/pools/session_lanes.rs
//! Что делает: решение 330 — события ГЛУБОКИХ пулов сессии пачки (pools/session.rs) обрабатываются полосами: у
//! каждого пула своя полоса (свой канал логов), события одного пула — строго по порядку прихода и по одному
//! (listen_log::on_log: применить событие или перечитать пул — off_map, реорг, расхождение, бины), а полосы разных
//! пулов идут одновременно отдельными задачами tokio — не больше dex.event_lanes_max сразу (расписание — lanes.rs).
//! Так перечитывание одного пула (Algebra — 100–380 мс на живом боте) не держит события остальных пулов пачки.
//! Действует для пулов со своей ссылкой (PoolRef::Contract, PoolRef::Singleton) ВСЕХ видов: лог такого пула находит
//! пул по индексу пачки (session.rs on_log). Пары общего контракта (WooFi, PoolRef::Member) и многомонетного пула
//! (PoolRef::Multi) и пробуждение мелкого пула — в сессии, как раньше (меняют подписку пачки и её очередь повтора).
//! Порядок и гонки:
//! • событие, пришедшее, пока пул перечитывается, ждёт в его полосе и применяется после (не теряется);
//! • один пул одновременно в одной полосе — двойного перечитывания одного пула нет;
//! • «ждёт ли пул повтора» (stale_read.rs waiting) берётся при запуске события, итог «опоздало» (StaleData) ставит
//!   пул в очередь повтора (requeue) при завершении — до запуска следующего события пула;
//! • новый блок (повтор и чтения каждым блоком) ждёт только полосы своих пулов (settle); приём новых пулов, пересинхрон
//!   ленты и смена настроек — после обработки всего принятого (drain), как раньше по очереди;
//! • обрыв сессии останавливает полосы (JoinSet), пулы — как раньше (недостоверны или держатся по
//!   dex.keep_pools_on_break), переподключение читает их заново.
//! Класс очереди узла — как раньше: задача полосы без класса (фон), перечитывание — dex.event_refresh_urgency.
//! Замер решения 313 (queued) — сколько событий ЭТОГО пула ждёт в его полосе.
//! Кто вызывает: pools/session.rs.
//! От чего зависит: lanes.rs, listen_log.rs (on_log), stale_read.rs, admit.rs (Members), group.rs (Msg), listen.rs
//! (Pools, End), load_batch_par.rs (limit), per_block.rs (needs).
//! Настройки: dex.event_lanes_max [bsc_dex] (необязательная; не задана — события по очереди в сессии, как раньше).

use std::collections::HashMap;

use alloy::rpc::types::Log;
use serde_json::value::RawValue;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::{JoinError, JoinSet};

use super::admit::Members;
use super::group::Msg;
use super::lanes::Lanes;
use super::listen::{End, Pools};
use super::{listen_log, load_batch_par, per_block, stale_read};

/// Настройка предела одновременных полос.
pub const MAX: &str = "dex.event_lanes_max";

/// Итог одного события полосы: индекс пула, канал полосы (возвращается на место), итог listen_log::on_log.
pub type Done = (usize, UnboundedReceiver<Msg>, Result<bool, End>);

/// Канал полосы пула: отправитель (сессия) и приёмник (у полосы; None — событие пула сейчас обрабатывается).
struct Chan {
    tx: UnboundedSender<Msg>,
    rx: Option<UnboundedReceiver<Msg>>,
}

/// Полосы событий пулов одной сессии пачки.
pub struct EventLanes {
    plan: Lanes<usize>,
    chans: HashMap<usize, Chan>,
    set: JoinSet<Done>,
}

impl Default for EventLanes {
    fn default() -> Self {
        EventLanes {
            plan: Lanes::new(),
            chans: HashMap::new(),
            set: JoinSet::new(),
        }
    }
}

impl EventLanes {
    /// Что делает: пустые полосы. Ошибок нет.
    pub fn new() -> Self {
        Self::default()
    }

    /// Что делает: отдавать ли лог глубокого пула полосам: задана dex.event_lanes_max или полосы ещё не пусты (после
    /// снятия настройки принятое дорабатывается полосами — порядок пула не ломается). Принимает: окружение.
    /// Возвращает: true — в полосу (push), false — по очереди в сессии, как раньше. Ошибок нет.
    pub fn wants(&self, p: &Pools) -> bool {
        !self.plan.idle() || load_batch_par::limit(&p.ctx, MAX).is_some()
    }

    /// Что делает: лог глубокого пула `i` — в конец его полосы (запуск — dispatch). Принимает: индекс пула, сырой лог.
    /// Ошибок нет.
    pub fn push(&mut self, i: usize, raw: Box<RawValue>) {
        let c = self.chans.entry(i).or_insert_with(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            Chan { tx, rx: Some(rx) }
        });
        // Приёмник у сессии (в карте) или у задачи полосы, которую держит JoinSet сессии, — отправка не падает.
        if c.tx.send(Msg::Log(raw)).is_ok() {
            self.plan.push(i);
        }
    }

    /// Что делает: запускает ждущие события в пределах dex.event_lanes_max (снята — 1, пока принятое не обработано).
    /// Принимает: окружение, пачку (очередь повтора — для waiting). Ошибки: End — лог не разобран, внутренняя ошибка.
    pub fn dispatch(&mut self, p: &Pools, m: &Members) -> Result<(), End> {
        let max = load_batch_par::limit(&p.ctx, MAX).unwrap_or(1);
        while let Some(i) = self.plan.next(max) {
            let mut rx = self
                .chans
                .get_mut(&i)
                .and_then(|c| c.rx.take())
                .ok_or_else(|| End::Broken("полоса пула без канала (внутренняя ошибка)".into()))?;
            let raw = match rx.try_recv() {
                Ok(Msg::Log(raw)) => raw,
                _ => return Err(End::Broken("полоса пула пуста (внутренняя ошибка)".into())),
            };
            let log: Log = serde_json::from_str(raw.get())
                .map_err(|e| End::Broken(format!("лог не разобран: {e}")))?;
            let waiting = stale_read::waiting(&m.missing, i);
            let p = p.clone();
            self.set.spawn(async move {
                let r = listen_log::on_log(&p, &log, waiting, &rx).await;
                (i, rx, r)
            });
        }
        Ok(())
    }

    /// Что делает: идёт ли сейчас хоть одна полоса (для ветки select! сессии). Ошибок нет.
    pub fn running(&self) -> bool {
        !self.set.is_empty()
    }

    /// Что делает: следующее завершённое событие полосы (отмена безопасна — JoinSet::join_next). None — полос нет.
    /// Ошибок нет.
    pub async fn next_done(&mut self) -> Option<Result<Done, JoinError>> {
        self.set.join_next().await
    }

    /// Что делает: завершение события: канал — на место, полоса свободна, «опоздало» (StaleData) — пул в очередь
    /// повтора (stale_read::requeue, как session.rs раньше). Принимает: пачку, итог. Ошибки: End — ошибка события
    /// (пул уже недостоверен — listen_log.rs) или задача полосы прервана.
    pub fn finish(&mut self, m: &mut Members, done: Result<Done, JoinError>) -> Result<(), End> {
        let (i, rx, r) =
            done.map_err(|e| End::Broken(format!("обработка события пула прервана: {e}")))?;
        if let Some(c) = self.chans.get_mut(&i) {
            c.rx = Some(rx);
        }
        self.plan.done(i);
        if r? {
            stale_read::requeue(&mut m.missing, &mut m.abandoned, i);
        }
        Ok(())
    }

    /// Что делает: ждёт, пока не останется выполняющихся событий пулов, для которых `hit` — да (новые не запускает).
    /// Принимает: пачку, отбор пулов. Ошибки: End — как у finish.
    pub async fn settle(
        &mut self,
        m: &mut Members,
        hit: impl Fn(usize) -> bool,
    ) -> Result<(), End> {
        while self.plan.running_any(&hit) {
            match self.set.join_next().await {
                Some(d) => self.finish(m, d)?,
                None => break,
            }
        }
        Ok(())
    }

    /// Что делает: перед повтором и чтениями каждым блоком (session.rs, новый блок) ждёт полосы пулов, которые эти
    /// чтения трогают: пулы очереди повтора (reload::retry) и виды «каждым блоком» (per_block::needs — с запасом, без
    /// отбора по USDT). Полосы остальных пулов идут дальше. Принимает: пачку. Ошибки: End — как у finish.
    pub async fn settle_block(&mut self, m: &mut Members) -> Result<(), End> {
        if !self.running() {
            return Ok(());
        }
        let hit: Vec<bool> = (0..m.pools.len())
            .map(|i| stale_read::waiting(&m.missing, i) || per_block::needs(m.pools[i].0.family))
            .collect();
        self.settle(m, |i| hit.get(i).copied().unwrap_or(true))
            .await
    }

    /// Что делает: обрабатывает всё принятое (запуская ждущие в пределах), пока полосы не опустеют. Принимает:
    /// окружение, пачку. Ошибки: End — как у dispatch и finish.
    pub async fn drain(&mut self, p: &Pools, m: &mut Members) -> Result<(), End> {
        loop {
            self.dispatch(p, m)?;
            match self.set.join_next().await {
                Some(d) => self.finish(m, d)?,
                None => return Ok(()),
            }
        }
    }
}
