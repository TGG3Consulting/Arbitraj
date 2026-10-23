//! Файл: venues/bsc_dex/wallet/inflows.rs
//! Что делает: не даёт ожидаемым поступлениям токена в кошелёк совпасть по времени с обменом, где этот токен — выход.
//! Проверка BALANCE_CHECK_ERC20 (решение 40) сравнивает баланс кошелька после обмена с «база + минимум», где база
//! читается прямо перед подписью обмена. Поступление того же токена между чтением базы и блоком обмена (депозит с
//! биржи, возврат USDT) завысило бы баланс, и обмен, давший меньше минимума, прошёл бы проверку. Поэтому:
//! - адрес депозита (контракт deposit_address) регистрирует ожидаемое поступление токена — но сначала ждёт, пока
//!   закончится обмен с этим токеном на выходе (иначе биржа начала бы перевод во время обмена);
//! - обмен (exec/trade.rs) сначала ждёт, пока ожидаемых поступлений его выходного токена нет, и держит отметку
//!   «идёт обмен» до занятия nonce в блоке;
//! - поступление снимается, когда статус депозита (deposit_status) — «зачислен» или «отклонён», либо по сроку
//!   transfer.max_time_s (перевод дольше — ядро ставит его в карантин; снятие — в журнал).
//!
//! Поступления, о которых бот не знает (чужой перевод на адрес бота), так не ловятся — факт сделки всё равно
//! сверяется по событиям Transfer квитанции (exec/result.rs), а у V2 минимум выхода проверяет сам роутер по балансу
//! получателя до и после обмена внутри транзакции.
//!
//! Кто вызывает: mod.rs (deposit_address, deposit_status), exec/trade.rs, exec/trade_smart.rs.
//! От чего зависит: ctx.rs.
//! Настройки: transfer.max_time_s [bsc_dex].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy_primitives::{Address, B256};
use tokio::sync::Notify;

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;

/// Состояние токена.
#[derive(Default)]
struct Tok {
    /// Когда зарегистрированы ожидаемые поступления.
    expected: Vec<Instant>,
    /// Идёт обмен с этим токеном на выходе.
    swapping: bool,
    /// Хеши уже учтённых поступлений: повторный запрос статуса не снимает чужое ожидание.
    finished: HashSet<B256>,
}

/// Ожидаемые поступления и идущие обмены. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Inflows {
    m: Arc<Mutex<HashMap<Address, Tok>>>,
    changed: Arc<Notify>,
}

/// Отметка «идёт обмен»: снимается при удалении.
pub struct SwapGuard {
    owner: Inflows,
    token: Address,
}

impl Inflows {
    /// Ждёт, пока ожидаемых поступлений токена нет, и ставит отметку «идёт обмен».
    /// Ошибки: NotConfigured (transfer.max_time_s).
    pub async fn swap(&self, ctx: &Ctx, token: Address) -> GatewayResult<SwapGuard> {
        let ttl = Duration::from_secs(ctx.int("transfer.max_time_s")?);
        loop {
            let wake = self.changed.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            let (expired, left) = {
                let mut m = self.m.lock().unwrap_or_else(|p| p.into_inner());
                let t = m.entry(token).or_default();
                let before = t.expected.len();
                t.expected.retain(|at| at.elapsed() < ttl);
                let expired = before - t.expected.len();
                if t.expected.is_empty() && !t.swapping {
                    t.swapping = true;
                    (expired, None)
                } else {
                    let first = t.expected.iter().min().copied();
                    (
                        expired,
                        Some(first.map(|f| ttl.saturating_sub(f.elapsed()))),
                    )
                }
            };
            if expired > 0 {
                let msg = format!(
                    "{token}: {expired} ожидаемых поступлений сняты по сроку transfer.max_time_s"
                );
                ctx.log(Level::Warn, "dex.inflow", msg, None).await;
            }
            match left {
                None => {
                    return Ok(SwapGuard {
                        owner: self.clone(),
                        token,
                    });
                }
                Some(Some(d)) => {
                    let _ = tokio::time::timeout(d, wake).await;
                }
                Some(None) => wake.await,
            }
        }
    }

    /// Ждёт конца обмена с этим токеном на выходе и регистрирует ожидаемое поступление. Ошибок нет.
    pub async fn expect(&self, token: Address) {
        loop {
            let wake = self.changed.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            {
                let mut m = self.m.lock().unwrap_or_else(|p| p.into_inner());
                let t = m.entry(token).or_default();
                if !t.swapping {
                    t.expected.push(Instant::now());
                    return;
                }
            }
            wake.await;
        }
    }

    /// Поступление с хешем tx закончилось (зачислено или отклонено): снимает самое раннее ожидание токена — один раз
    /// на хеш. Ошибок нет.
    pub fn done(&self, token: Address, tx: B256) {
        let mut m = self.m.lock().unwrap_or_else(|p| p.into_inner());
        let t = m.entry(token).or_default();
        if t.finished.insert(tx)
            && let Some(i) = t
                .expected
                .iter()
                .enumerate()
                .min_by_key(|(_, a)| **a)
                .map(|(i, _)| i)
        {
            t.expected.remove(i);
        }
        drop(m);
        self.changed.notify_waiters();
    }
}

impl Drop for SwapGuard {
    fn drop(&mut self) {
        let mut m = self.owner.m.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(t) = m.get_mut(&self.token) {
            t.swapping = false;
        }
        drop(m);
        self.owner.changed.notify_waiters();
    }
}
