//! Файл: venues/bsc_dex/node/events_lag.rs
//! Что делает: отметка «данные пулов отстают на N блоков» (решение 72): лента квитанций отстала больше
//! bsc.receipts_max_lag_blocks и догоняет без сброса — состояние пулов не выбрасывается (иначе перечитывание всех
//! пулов занимает минуты и следующий сброс приходит раньше), но котировка и сделка запрещены, пока лента не догнала
//! («не торгуем вслепую»). Одна отметка на площадку: ставит лента (node/receipts_task.rs), проверяют котировка
//! (quote/mod.rs) и сделка (exec/trade.rs).
//! Пока отметка стоит, сверка с квотером, пробы крючков и налогов вердикт не выносят (пропускают пул или монету) и
//! перепроверяют после её снятия — иначе ложные «расхождение с квотером» по старому состоянию.
//! Кто вызывает: node/receipts_task.rs, quote/mod.rs, exec/trade.rs, check/{quoter_cycle,cycle,taxes}.rs, parts.rs.
//! От чего зависит: ctx.rs.
//! Настройки: нет (порог — у ленты, bsc.receipts_max_lag_blocks).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;

/// Отставание данных пулов, блоков (0 — не отстают), и кого будить при снятии отметки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct EventsLag {
    blocks: Arc<AtomicU64>,
    wake: Arc<std::sync::Mutex<Vec<std::sync::Weak<tokio::sync::Notify>>>>,
}

impl EventsLag {
    /// Ставит отставание (0 — отметка снята). Снята после отставания — будит циклы проверок (`on_clear`): во время
    /// отставания они вердикт не выносили, теперь перепроверяют. Ошибок нет.
    pub fn set(&self, blocks: u64) {
        let was = self.blocks.swap(blocks, Ordering::SeqCst);
        if was > 0 && blocks == 0 {
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

    /// Текущее отставание. Ошибок нет.
    pub fn get(&self) -> u64 {
        self.blocks.load(Ordering::SeqCst)
    }

    /// Котировка и сделка: данные не отстают. Ошибки: StaleData «данные пулов отстают на N блоков».
    pub fn check(&self, ctx: &Ctx) -> GatewayResult<()> {
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
