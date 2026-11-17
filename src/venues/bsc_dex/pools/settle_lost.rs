//! Файл: venues/bsc_dex/pools/settle_lost.rs
//! Что делает: решение 357, ревью слияния «357 × 339» (Б2, Н4) — снятие отметок «в обновлении» (settle.rs), которые
//! своих логов уже не дождутся, БЕЗ возврата достоверности (пул вернёт только полное чтение):
//! • lost — лог пула из другого действия, чем ждёт отметка: Active — лог более позднего блока (логи блока отметки так и
//!   не дошли: поток подписки упорядочен по блокам); Follow — лог другого блока или другой транзакции (ожидаемый лог —
//!   из того же вызова: SwapFee → Swap — один `_emitSwapEvent` (cryptoalgebra 1ff43e1), CompositionFees →
//!   DepositedToBins — один mint (LBPair.sol v2.1.0), значит, та же транзакция). Было: Follow снимал любой следующий
//!   лог пула и возвращал was_valid — после обрыва сессии Swap следующего блока делал пул достоверным с потерянными
//!   событиями окна обрыва;
//! • forget_all — пачка недостоверна (listen_log.rs invalidate, обрыв сессии), пулы удалены из реестра (registry.rs
//!   retain);
//! • keep_only — пулы, которых больше нет среди найденных (market.rs, как отметки неудач решения 339): иначе
//!   осиротевшая отметка держала бы holds.any() навсегда (ветка голов у всех пачек, замок карты на каждую голову).
//! Кто вызывает: pools/listen_log.rs (lost, forget_all), registry.rs (forget_all), market.rs (keep_only).
//! От чего зависит: settle.rs (Holds, Need).
//! Настройки: нет.

use crate::core::types::PoolRef;

use super::settle::{Holds, Need};

impl Holds {
    /// Ревью слияния (Н4, Б2): отметки пулов `refs` — долой (удалены из реестра, пачка недостоверна). Ошибок нет.
    pub fn forget_all<'a>(&self, refs: impl IntoIterator<Item = &'a PoolRef>) {
        if self.any() {
            self.with(|m| {
                for r in refs {
                    m.remove(r);
                }
            });
        }
    }

    /// Ревью слияния (Н4): оставить отметки только пулов, для которых `keep` — да. Ошибок нет.
    pub fn keep_only(&self, keep: impl Fn(&PoolRef) -> bool) {
        if self.any() {
            self.with(|m| m.retain(|r, _| keep(r)));
        }
    }

    /// Ревью слияния (Б2): лог пула `r` (блок, транзакция) из ДРУГОГО действия, чем ожидает отметка «в обновлении»:
    /// Active — лог более позднего блока (логи блока отметки так и не дошли); Follow — лог другого блока или другой
    /// транзакции (ожидаемого лога того же вызова не было). Такой пул — только полным чтением. Возвращает: true —
    /// ожидание чужим логом (вызывающий снимает отметку forget и ставит пул на полное чтение). Ошибок нет.
    pub fn lost(&self, r: &PoolRef, block: u64, tx: Option<u64>) -> bool {
        self.get(r).is_some_and(|h| match h.need {
            Some(Need::Active(_)) => block != h.block,
            Some(Need::Follow) => block != h.block || tx != h.tx,
            // Решение 382: память неверна — любой лог пула ставит его на полное чтение сразу (не ждать головы).
            Some(Need::Reread) => true,
            None => false,
        })
    }
}
