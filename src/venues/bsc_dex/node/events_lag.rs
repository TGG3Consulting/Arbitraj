//! Файл: venues/bsc_dex/node/events_lag.rs
//! Что делает: отметка «данные пулов отстают на N блоков» (решение 72): лента квитанций отстала больше
//! bsc.receipts_max_lag_blocks и догоняет без сброса — состояние пулов не выбрасывается (иначе перечитывание всех
//! пулов занимает минуты и следующий сброс приходит раньше), но котировка и сделка запрещены, пока лента не догнала
//! («не торгуем вслепую»). Одна отметка на площадку: ставит лента (node/receipts_task.rs), проверяют котировка
//! (quote/mod.rs) и сделка (exec/trade.rs).
//! Пока отметка стоит, сверка с квотером, пробы крючков и налогов вердикт не выносят (пропускают пул или монету) и
//! перепроверяют после её снятия — иначе ложные «расхождение с квотером» по старому состоянию.
//! Полный сброс ленты (обрыв всех подписок) отметку НЕ снимает, а держит «сброс» (`hold`): пачки делают пулы
//! недостоверными (Invalidated) асинхронно, и до этого котировки и проверки шли бы по данным, отставшим на всё время
//! застоя. «Сброс» снимает лента (`release`), когда все «соединения» старой ленты закрыты — пачка закрывает своё только
//! после Invalidated (pools/session.rs); такое снятие циклы не будит (пулы перечитываются и дают «пул готов» сами).
//! Кто вызывает: node/receipts_task.rs, quote/mod.rs, exec/trade.rs, check/{quoter_cycle,cycle,taxes}.rs, parts.rs.
//! От чего зависит: ctx.rs.
//! Настройки: нет (порог — у ленты, bsc.receipts_max_lag_blocks).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;

/// Отставание данных пулов, блоков (0 — не отстают), и кого будить при снятии отметки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct EventsLag {
    blocks: Arc<AtomicU64>,
    /// Стоит «сброс»: лента сброшена, пачки ещё не сделали пулы недостоверными.
    held: Arc<AtomicBool>,
    /// С какого момента «сброс» стоит (для записи «запрет держится» — receipts_task.rs).
    held_since: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    wake: Arc<std::sync::Mutex<Vec<std::sync::Weak<tokio::sync::Notify>>>>,
}

impl EventsLag {
    /// Ставит отставание (0 — отметка снята). Снята после отставания — будит циклы проверок (`on_clear`): во время
    /// отставания они вердикт не выносили, теперь перепроверяют. Во время «сброса» не будит. Ошибок нет.
    pub fn set(&self, blocks: u64) {
        let was = self.blocks.swap(blocks, Ordering::SeqCst);
        if was > 0 && blocks == 0 && !self.held() {
            let mut wake = self.wake.lock().unwrap_or_else(|p| p.into_inner());
            // Подписки на рынок, которых уже нет, выпадают из списка.
            wake.retain(|w| w.strong_count() > 0);
            for n in wake.iter().filter_map(std::sync::Weak::upgrade) {
                n.notify_waiters();
            }
        }
    }

    /// Кого будить, когда отметка снята (сигнал «пул готов» подписки на рынок — pools/progress.rs). Ошибок нет.
    pub fn on_clear(&self, n: Arc<tokio::sync::Notify>) {
        self.wake
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::downgrade(&n));
    }

    /// Полный сброс ленты: «сброс» стоит, отставание не трогается. Ошибок нет.
    pub fn hold(&self) {
        let mut t = self.held_since.lock().unwrap_or_else(|p| p.into_inner());
        if !self.held.swap(true, Ordering::SeqCst) || t.is_none() {
            *t = Some(std::time::Instant::now());
        }
    }

    /// Снимает «сброс» (пулы старой ленты уже недостоверны), циклы не будит. Возвращает: true — стоял. Ошибок нет.
    pub fn release(&self) -> bool {
        *self.held_since.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.held.swap(false, Ordering::SeqCst)
    }

    /// Сколько «сброс» уже стоит. None — не стоит. Ошибок нет.
    pub fn held_for(&self) -> Option<std::time::Duration> {
        if !self.held() {
            return None;
        }
        self.held_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|t| t.elapsed())
    }

    /// Стоит «сброс». Ошибок нет.
    pub fn held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }

    /// Проверкам: вердикта нет — данные отстают или стоит «сброс». Ошибок нет.
    pub fn busy(&self) -> bool {
        self.get() > 0 || self.held()
    }

    /// Текущее отставание. Ошибок нет.
    pub fn get(&self) -> u64 {
        self.blocks.load(Ordering::SeqCst)
    }

    /// Котировка и сделка: данные не отстают и «сброса» нет. Ошибки: StaleData «данные пулов отстают на N блоков» или
    /// «лента сброшена».
    pub fn check(&self, ctx: &Ctx) -> GatewayResult<()> {
        if self.held() {
            return Err(ctx.err(
                ErrorCode::StaleData,
                "лента квитанций сброшена — пулы ещё не перечитаны, котировки и сделки нет"
                    .to_string(),
            ));
        }
        match self.get() {
            0 => Ok(()),
            n => Err(ctx.err(
                ErrorCode::StaleData,
                format!(
                    "данные пулов отстают на {n} блоков (лента квитанций догоняет без сброса) — котировки и сделки нет до догона"
                ),
            )),
        }
    }
}
