//! Файл: core/usdt_return_job.rs
//! Что делает: запуск перегона выручки закрытой сделки на площадку покупки (решение 138) — отдельная задача класса Trade
//! (путь сделки, решение 61), итог — в состояние сделки (usdt_return_state.rs): вернулись / не вернулись (сбой, исход
//! неизвестен). Задача оборвалась до итога (паника) — страховка «USDT не вернулись» из Drop.
//! Кто вызывает: core/usdt_return_deal.rs (prepare создаёт Job), core/ledger.rs (finish запускает после сохранения).
//! От чего зависит: usdt_return.rs (relay), usdt_return_state.rs, gateway/urgency.rs.
//! Настройки: через usdt_return.rs (usdt_return.*).

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::urgency::{self, Urgency};

use super::engine::Core;
use super::usdt_return_state::{self as st, UsdtReturn};

/// Начатый возврат: запускается после того, как закрытая сделка сохранена в учёте (иначе итог перегона мог бы
/// записаться в старую копию сделки раньше закрытия).
pub struct Job {
    pub(super) deal: String,
    pub(super) amount: Decimal,
    pub(super) from: VenueId,
    pub(super) to: VenueId,
    pub(super) acc: Option<DexAccountId>,
}

impl Job {
    /// Запускает перегон отдельной задачей класса Trade. Итог — в состояние сделки; задача оборвалась (паника) —
    /// страховка. Принимает: ядро. Ошибок нет.
    pub fn start(self, core: &Core) {
        let c = core.clone();
        tokio::spawn(urgency::scope(Urgency::Trade, async move {
            let mut guard = Guard {
                core: c.clone(),
                deal: self.deal.clone(),
                amount: self.amount,
                from: self.from.clone(),
                done: false,
            };
            let id = format!("{}u", self.deal);
            let res = super::usdt_return::relay(
                &c,
                &id,
                &self.from,
                &self.to,
                self.acc.as_ref(),
                self.amount,
            )
            .await;
            guard.done = true;
            match res {
                Ok(credited) => {
                    let note = format!(
                        "USDT вернулись: {} {} → {}, зачислено {credited}",
                        self.amount, self.from.0, self.to.0
                    );
                    let state = UsdtReturn::Done {
                        amount: self.amount,
                        credited,
                        from: self.from,
                        to: self.to,
                    };
                    st::set(&c, &self.deal, state, &note).await;
                }
                Err(e) if e.outcome_unknown() => {
                    let why = format!(
                        "исход перегона неизвестен ({e}) — проверьте обе площадки, перевод мог уйти"
                    );
                    st::mark_not_returned(&c, &self.deal, self.amount, &self.from, &why).await;
                }
                Err(e) => {
                    let why = format!("перегон не состоялся: {e}");
                    st::mark_not_returned(&c, &self.deal, self.amount, &self.from, &why).await;
                }
            }
        }));
    }
}

/// Страховка на случай, если задача перегона оборвалась (паника) до итога. Ошибок нет.
struct Guard {
    core: Core,
    deal: String,
    amount: Decimal,
    from: VenueId,
    done: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let Ok(h) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (c, d, a, f) = (
            self.core.clone(),
            self.deal.clone(),
            self.amount,
            self.from.clone(),
        );
        h.spawn(async move {
            let why =
                "задача перегона оборвалась до итога — проверьте обе площадки, перевод мог уйти";
            st::mark_not_returned(&c, &d, a, &f, why).await;
        });
    }
}
