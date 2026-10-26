//! Файл: core/credit_watch.rs
//! Что делает: ожидаемые зачисления переводов (решение 76). Перевод (transfers.rs) до вывода регистрирует ожидание:
//! получатель, монета, ожидаемый приход; хеш вывода — только от отправителя (withdrawal_status), когда он его отдал
//! (set_tx). События баланса (credit_event.rs) прикрепляются к ожиданию; закрывает ожидание только событие:
//! - с хешем — когда хеш отправителя уже известен и совпал (пока хеша нет — событие только будит опрос; с другим
//!   хешем оно снимается при set_tx и уходит во «внешнее изменение баланса»);
//! - без хеша (поток BingX, баланс BNB) — по сумме, при единственном подходящем ожидании (credit_event.rs);
//! - и только если у получателя transfer.credit_by_event = да (флаг на момент события).
//!
//! Ручной ввод и вывод, «Вернуть боту», «Беру на себя» ожиданий не создают — зачислением они не считаются.
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
    /// Хеш вывода от отправителя (нормализован — norm); None — ещё не известен.
    pub(super) tx: Option<String>,
    /// Прикреплённые события и закрывает ли каждое (transfer.credit_by_event на момент события).
    pub(super) hits: Vec<(BalanceChange, bool)>,
    pub(super) wake: Arc<Notify>,
}

impl Expect {
    /// Событие этого перевода: без хеша — да; с хешем — только если хеш отправителя известен и совпал.
    pub(super) fn ours(&self, b: &BalanceChange) -> bool {
        match (&b.tx_hash, &self.tx) {
            (None, _) => true,
            (Some(h), Some(tx)) => norm(h) == *tx,
            (Some(_), None) => false,
        }
    }
}

/// Хеш для сравнения: без «0x», в нижнем регистре. Ошибок нет.
pub fn norm(h: &str) -> String {
    h.trim_start_matches("0x").to_ascii_lowercase()
}

/// Ожидание перевода; при удалении снимается.
pub struct Ticket {
    core: Core,
    id: u64,
    pub wake: Arc<Notify>,
}

impl Ticket {
    /// Хеш вывода от отправителя известен. Возвращает: события с другим хешем — не этого перевода (сняты с
    /// ожидания; вызывающий пишет их во «внешнее изменение баланса»). Ошибок нет.
    pub fn set_tx(&self, tx: &str) -> Vec<BalanceChange> {
        let mut st = lock(&self.core);
        let Some(e) = st.list.get_mut(&self.id) else {
            return Vec::new();
        };
        e.tx = Some(norm(tx));
        let hits = std::mem::take(&mut e.hits);
        let (keep, foreign): (Vec<_>, Vec<_>) = hits.into_iter().partition(|(b, _)| e.ours(b));
        e.hits = keep;
        foreign.into_iter().map(|(b, _)| b).collect()
    }

    /// Событие, закрывающее ожидание (своё — Expect::ours — и закрытие включено). Ошибок нет.
    pub fn closing(&self) -> Option<BalanceChange> {
        let st = lock(&self.core);
        let e = st.list.get(&self.id)?;
        e.hits
            .iter()
            .find(|(b, close)| *close && e.ours(b))
            .map(|(b, _)| b.clone())
    }

    /// Пришло ли своё событие (закрывает оно или нет). Ошибок нет.
    pub fn has_event(&self) -> bool {
        let st = lock(&self.core);
        st.list
            .get(&self.id)
            .is_some_and(|e| e.hits.iter().any(|(b, _)| e.ours(b)))
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
        tx: None,
        hits: Vec::new(),
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
