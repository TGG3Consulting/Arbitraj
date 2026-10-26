//! Файл: core/credit_watch.rs
//! Что делает: зачисление перевода по событию, а не опросом статуса (решение 76). Перевод (transfers.rs) до вывода
//! регистрирует ожидаемое зачисление: получатель, монета, ожидаемый приход. Адаптеры шлют каждое изменение баланса
//! (MarketEvent::Balance); credit_event.rs сравнивает его с ожиданиями:
//! - поступление (Deposit) не своё, сумма в пределах transfer.credit_amount_tolerance_pct от ожидаемой — ровно одно
//!   ожидание подходит → оно отмечено; закрывает ожидание, только если у получателя transfer.credit_by_event = да
//!   (иначе — только запись в журнал, для сверки с опросом); подходят несколько — по событию не закрываем, решает опрос;
//! - вывод (Withdraw) с площадки, откуда сейчас идёт перевод этой монеты, — свой; иначе — постороннее;
//! - всё остальное не своё и поступление без ожидания — журнал «внешнее изменение баланса», без привязки к сделке.
//!   Ручной ввод и вывод, «Вернуть боту», «Беру на себя» ожиданий не создают — зачислением они не считаются.
//!
//! Кто вызывает: transfers.rs, transfer_credit.rs, credit_mismatch.rs (ожидание), credit_event.rs (сверка).
//! От чего зависит: engine.rs, core/types/balance.rs.
//! Настройки: нет (сверка — credit_event.rs).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::{Asset, BalanceChange, VenueId};

use super::engine::Core;

/// Ожидания и площадки, от которых приходят события. Одно на ядро.
#[derive(Default)]
pub struct Credits {
    m: Mutex<State>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) next: u64,
    pub(super) list: HashMap<u64, Expect>,
    /// Площадки, приславшие хоть одно событие баланса: только у них «статус зачислен, события нет» — расхождение.
    pub(super) seen: HashSet<VenueId>,
}

/// Одно ожидаемое зачисление.
pub(super) struct Expect {
    pub(super) deal: String,
    pub(super) from: VenueId,
    pub(super) to: VenueId,
    pub(super) asset: Asset,
    pub(super) amount: Decimal,
    /// Событие, подошедшее к ожиданию, и закрывает ли оно (transfer.credit_by_event на момент события).
    pub(super) hit: Option<(BalanceChange, bool)>,
    pub(super) wake: Arc<Notify>,
}

/// Ожидание перевода; при удалении снимается.
pub struct Ticket {
    core: Core,
    id: u64,
    pub wake: Arc<Notify>,
}

impl Ticket {
    /// Подошедшее событие и закрывает ли оно ожидание. Ошибок нет.
    pub fn hit(&self) -> Option<(BalanceChange, bool)> {
        lock(&self.core)
            .list
            .get(&self.id)
            .and_then(|e| e.hit.clone())
    }

    /// Присылала ли площадка события баланса с запуска. Ошибок нет.
    pub fn venue_seen(&self, v: &VenueId) -> bool {
        lock(&self.core).seen.contains(v)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        lock(&self.core).list.remove(&self.id);
    }
}

/// Состояние под замком. Ошибок нет.
pub(super) fn lock(core: &Core) -> std::sync::MutexGuard<'_, State> {
    core.inner
        .credits
        .m
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Регистрирует ожидаемое зачисление. Принимает: сделку, откуда, куда, монету, ожидаемый приход. Возвращает: ожидание
/// (снимается при удалении). Ошибок нет.
pub fn register(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    amount: Decimal,
) -> Ticket {
    let wake = Arc::new(Notify::new());
    let mut st = lock(core);
    st.next += 1;
    let id = st.next;
    let e = Expect {
        deal: deal.to_string(),
        from: from.clone(),
        to: to.clone(),
        asset: asset.clone(),
        amount,
        hit: None,
        wake: wake.clone(),
    };
    st.list.insert(id, e);
    Ticket {
        core: core.clone(),
        id,
        wake,
    }
}

/// Тот же актив: тикер совпадает, и адреса контрактов совпадают, если известны оба. Ошибок нет.
pub fn same_asset(a: &Asset, b: &Asset) -> bool {
    a.ticker == b.ticker
        && (a.bsc_contract.is_none()
            || b.bsc_contract.is_none()
            || a.bsc_contract == b.bsc_contract)
}
