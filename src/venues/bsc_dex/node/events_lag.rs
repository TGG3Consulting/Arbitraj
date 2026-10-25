//! Файл: venues/bsc_dex/node/events_lag.rs
//! Что делает: отметка «данные пулов отстают на N блоков» (решение 72): лента квитанций отстала больше
//! bsc.receipts_max_lag_blocks и догоняет без сброса — состояние пулов не выбрасывается (иначе перечитывание всех
//! пулов занимает минуты и следующий сброс приходит раньше), но котировка и сделка запрещены, пока лента не догнала
//! («не торгуем вслепую»). Одна отметка на площадку: ставит лента (node/receipts_task.rs), проверяют котировка
//! (quote/mod.rs) и сделка (exec/trade.rs).
//! Кто вызывает: node/receipts_task.rs, quote/mod.rs, exec/trade.rs, parts.rs (создаёт).
//! От чего зависит: ctx.rs.
//! Настройки: нет (порог — у ленты, bsc.receipts_max_lag_blocks).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;

/// Отставание данных пулов, блоков (0 — не отстают). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct EventsLag(Arc<AtomicU64>);

impl EventsLag {
    /// Ставит отставание (0 — догнали). Ошибок нет.
    pub fn set(&self, blocks: u64) {
        self.0.store(blocks, Ordering::SeqCst);
    }

    /// Текущее отставание. Ошибок нет.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
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
