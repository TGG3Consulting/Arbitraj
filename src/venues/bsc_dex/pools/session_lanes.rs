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
//! • обрыв сессии останавливает полосы и ЖДЁТ их остановки (shutdown, ревью t153 Н1) — до того, как сессия объявит
//!   пулы пачки недостоверными (session.rs run → invalidate): задача полосы не запишет пул достоверным после этого;
//!   пулы — как раньше (недостоверны или держатся по dex.keep_pools_on_break), переподключение читает их заново;
//! • событие, которое не удалось положить в полосу, — явный обрыв сессии (End::Broken, ревью t153 Н2), не молча;
//! • итог true (опоздало, ошибка пула при 339 или событие недостоверного пула — ревью t153 Б1) ставит пул в очередь
//!   повтора через pool_fail::requeue (при 339 брошенный из-за незаданной настройки остаётся брошенным).
//! Класс очереди узла — как раньше: задача полосы без класса (фон), перечитывание — dex.event_refresh_urgency.
//! Замер решения 313 (queued) — сколько событий ЭТОГО пула ждёт в его полосе (при полосах; без полос — как раньше,
//! сколько сообщений ждёт в канале всей пачки: числа «до» и «после» включения полос не сравнимы — ревью t153 Н8).
//! Кто вызывает: pools/session.rs.
//! От чего зависит: lanes.rs, listen_log.rs (on_log), stale_read.rs, pool_fail.rs (requeue, base), admit.rs (Members),
//! group.rs (Msg), listen.rs (Pools, End), load_batch_par.rs (limit), per_block.rs (needs), settle.rs (решение 357 —
//! отметки пулов «в обновлении»: settle_block ждёт и их полосы, queued — для settle_close.rs).
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
use super::{listen_log, load_batch_par, per_block, pool_fail, stale_read};

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
    /// Решение 339 включено (pool_fail::base) — по последнему dispatch; для requeue в finish (ревью t153 Б1).
    isolate: bool,
}

impl Default for EventLanes {
    fn default() -> Self {
        EventLanes {
            plan: Lanes::new(),
            chans: HashMap::new(),
            set: JoinSet::new(),
            isolate: false,
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
    /// Ошибки: End::Broken — канал полосы закрыт (по коду недостижимо: приёмник у сессии в карте или у задачи полосы в
    /// JoinSet сессии; ревью t153 Н2 — событие не теряется молча, сессия переподключается и перечитывает пулы).
    pub fn push(&mut self, i: usize, raw: Box<RawValue>) -> Result<(), End> {
        let c = self.chans.entry(i).or_insert_with(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            Chan { tx, rx: Some(rx) }
        });
        if c.tx.send(Msg::Log(raw)).is_err() {
            return Err(End::Broken(
                "событие пула не положено в его полосу — канал полосы закрыт (внутренняя ошибка)"
                    .into(),
            ));
        }
        self.plan.push(i);
        Ok(())
    }

    /// Что делает: запускает ждущие события в пределах dex.event_lanes_max (снята — 1, пока принятое не обработано).
    /// Принимает: окружение, пачку (очередь повтора — для waiting). Ошибки: End — лог не разобран, внутренняя ошибка.
    pub fn dispatch(&mut self, p: &Pools, m: &Members) -> Result<(), End> {
        let max = load_batch_par::limit(&p.ctx, MAX).unwrap_or(1);
        while let Some(i) = self.plan.next(max) {
            // Ревью t153 (Б1): включено ли 339 — для requeue в finish; читается, только когда событие запускается.
            self.isolate = pool_fail::base(&p.ctx).is_some();
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

    /// Что делает: решение 357 (слияние с t154) — сколько событий пула `i` ещё в его полосе: ждут запуска плюс
    /// выполняющееся (для settle_close.rs: логи пула «в пути»). Ошибок нет.
    pub fn queued(&self, i: usize) -> usize {
        self.plan.waiting(i) + usize::from(self.plan.running_any(|k| k == i))
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

    /// Что делает: завершение события: канал — на место, полоса свободна, итог true (StaleData, ошибка пула при 339,
    /// событие недостоверного пула — ревью t153 Б1) — пул в очередь повтора (pool_fail::requeue). Принимает: пачку, итог. Ошибки: End — ошибка события
    /// (пул уже недостоверен — listen_log.rs) или задача полосы прервана.
    pub fn finish(&mut self, m: &mut Members, done: Result<Done, JoinError>) -> Result<(), End> {
        let (i, rx, r) =
            done.map_err(|e| End::Broken(format!("обработка события пула прервана: {e}")))?;
        if let Some(c) = self.chans.get_mut(&i) {
            c.rx = Some(rx);
        }
        self.plan.done(i);
        if r? {
            pool_fail::requeue(self.isolate, &mut m.missing, &mut m.abandoned, i);
        }
        Ok(())
    }

    /// Что делает: ревью t153 (Н1) — обрыв сессии: прервать задачи полос и ДОЖДАТЬСЯ их остановки (JoinSet::shutdown),
    /// чтобы ни одна не записала пул после того, как сессия объявит пачку недостоверной. Ошибок нет.
    pub async fn shutdown(&mut self) {
        self.set.shutdown().await;
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
    /// чтения трогают: пулы очереди повтора (reload::retry), виды «каждым блоком» (per_block::needs — с запасом, без
    /// отбора по USDT) и пулы с отметкой решения 357 (settle.rs: их может прочитать целиком settle_close.rs на этой же
    /// голове — не одновременно с полосой того же пула). Полосы остальных пулов идут дальше. Принимает: окружение,
    /// пачку. Ошибки: End — как у finish.
    pub async fn settle_block(&mut self, p: &Pools, m: &mut Members) -> Result<(), End> {
        if !self.running() {
            return Ok(());
        }
        let hit: Vec<bool> = (0..m.pools.len())
            .map(|i| {
                stale_read::waiting(&m.missing, i)
                    || per_block::needs(m.pools[i].0.family)
                    || per_block::ramping(p, &m.pools[i].0)
                    || p.holds.get(&m.pools[i].0.reference).is_some()
            })
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
