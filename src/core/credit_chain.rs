//! Файл: core/credit_chain.rs
//! Что делает: решение 151 — зачисление перевода сделки на кошелёк DEX по приходу монеты в блокчейне, не дожидаясь,
//! пока биржа-отправитель отдаст в API хеш и статус вывода (решение 76 остаётся для всего прочего: перегонов, возврата
//! USDT, биржа ↔ биржа, переводов на биржу). Ожидание зачисления (credit_watch.rs) «вооружается» только у перевода
//! сделки на DEX (transfers.rs send_trade → arm); поступление (credit_event.rs) засчитывается зачислением, если ВСЕ:
//! - transfer.credit_by_chain = да у получателя (на момент события);
//! - кошелёк события — кошелёк сделки (счёт DEX из маппинга биржи-отправителя, решение 132), известен у обоих;
//! - токен — по адресу контракта монеты сделки (известен у обоих и совпал);
//! - количество — ожидаемый приход (вывод минус комиссия вывода в монете, credit_expect.rs) в допуске
//!   transfer.credit_amount_tolerance_pct (тот же допуск сверки прихода, credit_match.rs);
//! - на этом кошельке по этому токену нет другого ожидаемого прихода (другой сделки, перегона, возврата);
//! - момент события (Transfer после нужных подтверждений) не раньше отправки запроса вывода;
//! - хеш вывода от отправителя ещё не известен (известен — обычная сверка по хешу, решение 76).
//!
//! Горячие кошельки бирж не проверяются — их нет в API бирж (не придумываем).
//!
//! Кто вызывает: transfers.rs (arm, Ticket::chain_hit), credit_event.rs (accept).
//! От чего зависит: credit_watch.rs (State, Expect, Ticket), credit_match.rs, engine.rs (виды площадок).
//! Настройки: transfer.credit_by_chain (читает credit_event.rs), transfer.credit_amount_tolerance_pct.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{BalanceChange, VenueId};
use crate::gateway::contract::VenueKind;

use super::credit_watch::{State, Ticket, lock, same_asset};
use super::engine::{Core, rd};

/// Итог проверки поступления на зачисление по блокчейну.
pub(super) enum Verdict {
    /// Не применяется: перевод не «вооружён» (не сделка на DEX) или хеш отправителя уже известен.
    Off,
    /// Засчитано зачислением.
    Taken,
    /// Не засчитано — причина (журнал).
    No(String),
}

impl Ticket {
    /// Решение 151: ожидание перевода сделки на DEX может закрыться приходом в блокчейне. Принимает: момент отправки
    /// запроса вывода (поступление раньше — не наше). Ошибок нет.
    pub fn set_chain(&self, since: DateTime<Utc>) {
        if let Some(e) = lock(&self.core).list.get_mut(&self.id) {
            e.chain_since = Some(since);
        }
    }

    /// Поступление, засчитанное зачислением по блокчейну (решение 151). None — ещё нет. Ошибок нет.
    pub fn chain_hit(&self) -> Option<BalanceChange> {
        lock(&self.core)
            .list
            .get(&self.id)
            .and_then(|e| e.chain_hit.clone())
    }
}

/// «Вооружает» ожидание перевода сделки (решение 151): только если получатель — DEX и счёт сделки известен.
/// Принимает: ядро, ожидание (None — событий нет, только опрос), получатель, есть ли счёт DEX, момент вывода.
/// Возвращает: вооружено ли. Ошибок нет.
pub fn arm(
    core: &Core,
    ticket: Option<&Ticket>,
    to: &VenueId,
    has_account: bool,
    since: DateTime<Utc>,
) -> bool {
    let dex = rd(&core.inner.kinds)
        .iter()
        .any(|(v, k)| v == to && *k == VenueKind::Dex);
    match ticket {
        Some(t) if dex && has_account => {
            t.set_chain(since);
            true
        }
        _ => false,
    }
}

/// Проверяет поступление `b` к единственному подходящему по сумме ожиданию `id` (credit_event.rs, Pick::One) и при
/// успехе отмечает его зачисленным (Expect::chain_hit) и будит перевод. Принимает: состояние, id, событие, допуск,
/// включено ли (transfer.credit_by_chain). Возвращает: Verdict. Ошибок нет.
pub(super) fn accept(
    st: &mut State,
    id: u64,
    b: &BalanceChange,
    tol: Decimal,
    on: bool,
) -> Verdict {
    let Some(e) = st.list.get(&id) else {
        return Verdict::Off;
    };
    let (Some(since), None, true) = (e.chain_since, &e.tx, on) else {
        return Verdict::Off;
    };
    if b.tx_hash.is_none() {
        return Verdict::No("в событии нет хеша транзакции".into());
    }
    if e.account.is_none() || e.account != b.account {
        return Verdict::No("не кошелёк сделки".into());
    }
    if e.asset.bsc_contract.is_none() || e.asset.bsc_contract != b.asset.bsc_contract {
        return Verdict::No("токен не по адресу контракта монеты сделки".into());
    }
    if !super::credit_match::fits(e.amount, b.delta, tol) {
        return Verdict::No(format!("не ожидаемое количество {}", e.amount));
    }
    if b.at < since {
        return Verdict::No("приход раньше запроса вывода".into());
    }
    let others = st
        .list
        .iter()
        .filter(|(k, x)| **k != id && x.to == b.venue && same_asset(&x.asset, &b.asset))
        .filter(|(_, x)| x.same_wallet(b))
        .count();
    if others > 0 {
        return Verdict::No(format!(
            "на кошелёк ожидается ещё {others} поступлений {}",
            b.asset.ticker
        ));
    }
    if let Some(e) = st.list.get_mut(&id) {
        e.chain_hit = Some(b.clone());
        e.wake.notify_one();
    }
    Verdict::Taken
}

/// Строка хронологии сделки (решение 151). Принимает: отправитель, количество, тикер, хеш. Ошибок нет.
pub fn note(from: &VenueId, qty: Decimal, ticker: &str, hash: &str) -> String {
    format!(
        "Зачислено по блокчейну (не дожидаясь API {}): {qty} {ticker}, хеш {hash}",
        from.0
    )
}
