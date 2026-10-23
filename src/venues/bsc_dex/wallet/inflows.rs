//! Файл: venues/bsc_dex/wallet/inflows.rs
//! Что делает: не даёт ожидаемым поступлениям токена в кошелёк совпасть по времени с обменом, где этот токен — выход.
//! Проверка BALANCE_CHECK_ERC20 (решение 40) сравнивает баланс кошелька после обмена с «база + минимум», где база
//! читается прямо перед подписью обмена. Поступление того же токена между чтением базы и блоком обмена (депозит с
//! биржи, возврат USDT) завысило бы баланс, и обмен, давший меньше минимума, прошёл бы проверку. Поэтому:
//! - адрес депозита (контракт deposit_address) регистрирует ожидаемое поступление токена — но сначала ждёт, пока
//!   закончится обмен с этим токеном на выходе (иначе биржа начала бы перевод во время обмена);
//! - обмен (exec/trade.rs) сначала ждёт, пока ожидаемых поступлений его выходного токена нет (причина ожидания —
//!   одной записью в журнал), и держит отметку «идёт обмен» до занятия nonce в блоке;
//! - ожидание снимается при любом завершении вывода: статус депозита «зачислен» / «отклонён» / хеш или квитанция
//!   не разбираются (один раз на хеш), вывод не состоялся (контракт deposit_abandoned — ядро, core/transfers.rs),
//!   либо по сроку transfer.max_time_s (ядро ставит такой перевод в карантин; снятие — в журнал).
//!
//! Поступления, о которых бот не знает (чужой перевод на адрес бота), так не ловятся — факт сделки всё равно
//! сверяется по событиям Transfer квитанции (exec/result.rs), а у V2 минимум выхода проверяет сам роутер по балансу
//! получателя до и после обмена внутри транзакции.
//!
//! Кто вызывает: mod.rs (deposit_address, deposit_status, deposit_abandoned), exec/trade.rs, exec/trade_smart.rs.
//! От чего зависит: ctx.rs.
//! Настройки: transfer.max_time_s [bsc_dex].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy_primitives::Address;
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
    /// Уже учтённые завершения (хеш → когда): повторный запрос статуса не снимает чужое ожидание. Чистятся по сроку.
    finished: HashMap<String, Instant>,
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
        let mut told = false;
        loop {
            let wake = self.changed.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            let (expired, left) = {
                let mut m = self.m.lock().unwrap_or_else(|p| p.into_inner());
                let t = m.entry(token).or_default();
                let expired = purge(t, ttl);
                if t.expected.is_empty() && !t.swapping {
                    t.swapping = true;
                    (expired, None)
                } else {
                    let first = t.expected.iter().min().copied();
                    let wait = first.map(|f| ttl.saturating_sub(f.elapsed()));
                    (expired, Some((t.expected.len(), wait)))
                }
            };
            if expired > 0 {
                let msg = format!(
                    "{token}: {expired} ожидаемых поступлений сняты по сроку transfer.max_time_s"
                );
                ctx.log(Level::Warn, "dex.inflow", msg, None).await;
            }
            let Some((n, wait)) = left else {
                return Ok(SwapGuard {
                    owner: self.clone(),
                    token,
                });
            };
            if !told && n > 0 {
                told = true;
                let msg = format!(
                    "Обмен ждёт: в кошелёк ожидается {n} поступлений {token} (адрес депозита выдан) — база проверки баланса была бы неверной; ждём зачисления, отказа вывода или срока transfer.max_time_s"
                );
                ctx.log(Level::Info, "dex.inflow", msg, None).await;
            }
            match wait {
                Some(d) => {
                    let _ = tokio::time::timeout(d, wake).await;
                }
                None => wake.await,
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

    /// Поступление с ключом `key` (хеш вывода) закончилось — зачислено, отклонено или не разбирается: снимает самое
    /// раннее ожидание токена, один раз на ключ. Ошибок нет.
    pub fn done(&self, token: Address, key: &str) {
        let mut m = self.m.lock().unwrap_or_else(|p| p.into_inner());
        let t = m.entry(token).or_default();
        if !t.finished.contains_key(key) {
            t.finished.insert(key.to_string(), Instant::now());
            remove_first(t);
        }
        drop(m);
        self.changed.notify_waiters();
    }

    /// Вывод на кошелёк не состоялся (ядро больше не ждёт депозит): снимает самое раннее ожидание токена. Ошибок нет.
    pub fn abandoned(&self, token: Address) {
        let mut m = self.m.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(t) = m.get_mut(&token) {
            remove_first(t);
        }
        drop(m);
        self.changed.notify_waiters();
    }
}

/// Снимает самое раннее ожидание. Ошибок нет.
fn remove_first(t: &mut Tok) {
    if let Some(i) = t
        .expected
        .iter()
        .enumerate()
        .min_by_key(|(_, a)| **a)
        .map(|(i, _)| i)
    {
        t.expected.remove(i);
    }
}

/// Снимает ожидания и учтённые завершения старше срока. Возвращает: сколько ожиданий снято. Ошибок нет.
fn purge(t: &mut Tok, ttl: Duration) -> usize {
    let before = t.expected.len();
    t.expected.retain(|at| at.elapsed() < ttl);
    t.finished.retain(|_, at| at.elapsed() < ttl);
    before - t.expected.len()
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
