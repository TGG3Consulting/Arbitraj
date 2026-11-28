//! Файл: venues/bsc_dex/directory/boot.rs
//! Что делает: ревью арбитра 28.11.2026, С2 — окончен ли «первый проход справочника» подписки: первый проход обновления
//! от индексатора (directory/refresh.rs) и первый проход поиска по фабрикам (pools/factory_scan.rs) завершились (с итогом,
//! ошибкой или выключены — тогда проход короткий), и все монеты, которые они отправили в канал «монета обновлена»,
//! обработаны слушателем (pools/index_watch.rs: проверка на узле и подключение пулов — attach кладёт их в «загружаются»,
//! pools/loading.rs). До этого сводки первого прохода (налоги — check/tax_wait.rs First, сверка с квотером и крючки —
//! check/fresh.rs settled) не уходят: живой бот 28.11.2026 (base_dex) — справочник индексатора пуст, сводки и CheckPass
//! ядру ушли при нуле пулов, до прихода 3025 пулов от поиска по фабрикам.
//! Счёт: источник увеличивает «отправлено» ДО отправки (send), слушатель пишет «получено» — сколько сообщений канала
//! он забрал (index_retry.rs), — после обработки (handled). Канал один и FIFO, поэтому «получено ≥ отправлено» значит,
//! что всё отправленное обработано. Начало подписки (start) сбрасывает счёт; до первого start (тесты, площадка без
//! подписки) — проход считается оконченным, как раньше. Стало окончено — «пул готов» (сигнал подписки): циклы проверок
//! пересчитывают сводку сразу.
//! Кто вызывает: pools/market.rs (start), directory/refresh.rs и pools/factory_scan.rs (send, source_done),
//! pools/index_watch.rs (handled), check/fresh.rs и check/tax_cycle.rs (complete).
//! От чего зависит: ctx.rs (справочник — Snap.boot), tokio Notify.
//! Настройки: нет.

use std::sync::Arc;

use alloy_primitives::Address;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;

use super::super::ctx::Ctx;

/// Источник справочника.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Обновление от индексатора (directory/refresh.rs).
    Indexer,
    /// Поиск по фабрикам на своём узле (pools/factory_scan.rs).
    Factory,
}

/// Состояние первого прохода справочника подписки (Snap.boot).
#[derive(Debug, Default)]
pub struct Boot {
    /// Подписка началась (start) — без этого проход считается оконченным.
    armed: bool,
    indexer: bool,
    factory: bool,
    /// Монет отправлено источниками в канал «монета обновлена».
    sent: u64,
    /// Сообщений канала обработано слушателем.
    done: u64,
    /// «Пул готов» подписки — будить циклы проверок, когда проход окончен.
    wake: Option<Arc<Notify>>,
}

impl Boot {
    /// Окончен ли первый проход (см. шапку). Ошибок нет.
    pub fn complete(&self) -> bool {
        !self.armed || (self.indexer && self.factory && self.done >= self.sent)
    }
}

/// Начало подписки: счёт заново, сигнал «пул готов» этой подписки. Принимает: окружение, сигнал. Ошибок нет.
pub fn start(ctx: &Ctx, wake: Arc<Notify>) {
    ctx.index.with(|s| {
        s.boot = Boot {
            armed: true,
            wake: Some(wake),
            ..Boot::default()
        }
    });
}

/// Отправка монеты в канал «монета обновлена» со счётом (до отправки). Канал закрыт — подписки нет, счёт не важен.
/// Ошибок нет.
pub fn send(ctx: &Ctx, tx: &UnboundedSender<Address>, coin: Address) {
    ctx.index.with(|s| s.boot.sent += 1);
    let _ = tx.send(coin);
}

/// Первый (и каждый следующий) проход источника окончен. Ошибок нет.
pub fn source_done(ctx: &Ctx, src: Source) {
    change(ctx, |b| match src {
        Source::Indexer => b.indexer = true,
        Source::Factory => b.factory = true,
    });
}

/// Слушатель обработал `received` сообщений канала с начала подписки. Ошибок нет.
pub fn handled(ctx: &Ctx, received: u64) {
    change(ctx, |b| b.done = b.done.max(received));
}

/// Окончен ли первый проход справочника подписки. Ошибок нет.
pub fn complete(ctx: &Ctx) -> bool {
    ctx.index.with(|s| s.boot.complete())
}

/// Изменение счёта; стало окончено — «пул готов» (вне замка). Ошибок нет.
fn change(ctx: &Ctx, f: impl FnOnce(&mut Boot)) {
    let wake = ctx.index.with(|s| {
        let was = s.boot.complete();
        f(&mut s.boot);
        (!was && s.boot.complete())
            .then(|| s.boot.wake.clone())
            .flatten()
    });
    if let Some(n) = wake {
        n.notify_waiters();
    }
}
