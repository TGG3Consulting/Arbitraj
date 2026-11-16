//! Файл: core/stuck_lag.rs
//! Что делает: решение 334 (задача t155) — ожидание тика монеты застрявшей сделки (Ticks ниже: режим
//! «Ожидание», «Перегон» — relay_here.rs) отстало от рассылки тиков ядра (broadcast coin_ticks, ёмкость --live-buffer;
//! RecvError::Lagged — пропущено n тиков, среди них мог быть тик ЭТОЙ монеты). Раньше — молча ждали следующего тика
//! монеты (у тихой монеты — до срока stuck.wait_ms, а проверка «стало выгодно» пропускалась).
//! - Счёт и запись журнала core.coin_ticks_lost (gateway/lost_log.rs: на 1-ю, 2-ю, 4-ю… или по интервалу).
//! - Отставание считается возможным тиком монеты — проверка выгодности сразу (лишняя проверка безвредна: она та же,
//!   что на настоящий тик; Lagged бывает раз на переполнение подписки — цикла проверок нет). Решение 361 (в логику,
//!   правило 16.11): без настройки-выключателя (была stuck.tick_lag_wake).
//!
//! Ревью 333, Н4: ожидание — Ticks, подписка на рассылку тиков ОДНА на всё ожидание (раньше — новая на каждый вызов
//! next_tick: тики, пришедшие, пока шла котировка между вызовами, терялись молча, и Lagged их не показывал). Теперь они
//! копятся в подписке и следующий next отдаёт их сразу (ёмкость — та же --live-buffer; переполнение — Lagged выше).
//!
//! Кто вызывает: core/stuck.rs («Ожидание», «Порог убытка»), core/relay_here.rs (quote_or_wait).
//! От чего зависит: engine.rs (coin_ticks), deal.rs, gateway/lost_log.rs, settings, tokio (broadcast, timeout_at).
//! Настройки: журнал — log.events_lost_interval_s.

use std::sync::LazyLock;

use tokio::sync::broadcast::{Receiver, error::RecvError};
use tokio::time::Instant;

use crate::core::types::Asset;
use crate::gateway::lost_log::{Lost, Note};

use super::deal::Deal;
use super::engine::Core;

/// Вид записи журнала.
pub const LOST_KIND: &str = "core.coin_ticks_lost";

/// Счёт отставаний (один на процесс: рассылка тиков одна).
static LOST: LazyLock<Lost> = LazyLock::new(Lost::default);

/// Отставание ожидания тика: счёт и журнал (проверка сразу — решает Ticks::next). Принимает: ядро, сделку, сколько
/// тиков пропущено. Ошибок нет.
pub fn lagged(core: &Core, deal: &Deal, n: u64) {
    let deps = &core.inner.deps;
    let note = Note {
        kind: LOST_KIND,
        venue: None,
        what: "ожидание тика монеты застрявшей сделки отстало от рассылки тиков ядра — тик монеты мог пропасть",
        last: format!(
            "{} (сделка {}), пропущено тиков {n}",
            deal.direction.asset.ticker, deal.id
        ),
    };
    LOST.hit(&deps.settings, &deps.journal, note);
}

/// Подписка на тики монет на всё ожидание застрявшей сделки (Н4).
pub struct Ticks(Receiver<Asset>);

impl Ticks {
    /// Подписка с этого момента. Принимает: ядро. Ошибок нет.
    pub fn new(core: &Core) -> Self {
        Self(core.inner.coin_ticks.subscribe())
    }

    /// Ждёт тика монеты сделки (или времени `until`); тики, пришедшие с прошлого вызова, не теряются. Отставание —
    /// lagged (счёт, журнал) и проверка сразу (решение 361). Принимает: ядро, сделку, срок. Возвращает: true — тик или
    /// отставание (тик монеты мог пропасть); false — время вышло или рассылка закрыта. Ошибок нет.
    pub async fn next(&mut self, core: &Core, deal: &Deal, until: Option<Instant>) -> bool {
        loop {
            let recv = self.0.recv();
            let got = match until {
                Some(t) => match tokio::time::timeout_at(t, recv).await {
                    Ok(r) => r,
                    Err(_) => return false,
                },
                None => recv.await,
            };
            match got {
                Ok(a) if a == deal.direction.asset => return true,
                Ok(_) => continue,
                // Решения 334, 361: подписчик отстал — тик этой монеты мог пропасть: счёт, журнал и проверка сразу.
                Err(RecvError::Lagged(n)) => {
                    lagged(core, deal, n);
                    return true;
                }
                Err(RecvError::Closed) => return false,
            }
        }
    }
}
