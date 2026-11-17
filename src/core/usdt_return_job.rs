//! Файл: core/usdt_return_job.rs
//! Что делает: запуск перегона выручки закрытой сделки на площадку покупки (решение 138) — отдельная задача класса Trade
//! (путь сделки, решение 61), итог — в состояние сделки (usdt_return_state.rs): вернулись / не вернулись (сбой, исход
//! неизвестен). Задача оборвалась до итога (паника) — страховка «USDT не вернулись» из Drop. Решение 163: сбой, при
//! котором вывод точно не ушёл, и заданы повторы (usdt.return_retry_s, usdt.return_retries_max) — повторы
//! (usdt_return_retry.rs); тогда сбой первой попытки не извещается usdt.relay_failed — итог известит повтор.
//! Решение 227: Job — одна ЧАСТЬ возврата (площадка выручки); у сделки с продажами на нескольких площадках — по Job на
//! часть, итог каждой — в её состояние.
//! Решение 394 (ревью 3 t179, Б1): Job общего возврата группы докупки выше максимума вывода несёт куски (pieces) —
//! они уходят по очереди в этой же задаче и одной части учёта (usdt_return_pieces.rs); без кусков — как раньше.
//! Кто вызывает: core/usdt_return_deal.rs (prepare создаёт Job), core/ledger.rs (finish запускает после сохранения).
//! От чего зависит: usdt_return.rs (relay, relay_try), usdt_return_retry.rs, usdt_return_state.rs, gateway/urgency.rs.
//! Настройки: через usdt_return.rs (usdt_return.*), через usdt_return_retry.rs (usdt.return_retry_s, usdt.return_retries_max).

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::urgency::{self, Urgency};

use super::engine::Core;
use super::usdt_return_state as st;

/// Начатый возврат: запускается после того, как закрытая сделка сохранена в учёте (иначе итог перегона мог бы
/// записаться в старую копию сделки раньше закрытия).
pub struct Job {
    pub(super) deal: String,
    pub(super) amount: Decimal,
    pub(super) from: VenueId,
    pub(super) to: VenueId,
    pub(super) acc: Option<DexAccountId>,
    /// Решение 394 (ревью 3, Б1): куски общего возврата группы по очереди (сумма = amount); пусто — один вывод.
    pub(super) pieces: Vec<Decimal>,
    /// Решение 394 (ревью 4, Н8): метка попытки в ID вывода кусков («» — шаг закрытия, «m<мс>» — «Перегнать»).
    pub(super) tag: String,
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
            // Решение 394 (ревью 3, Б1): куски — по очереди, одна часть учёта (usdt_return_pieces.rs).
            if self.pieces.len() > 1 {
                return Box::pin(super::usdt_return_pieces::run(&c, &self, &mut guard)).await;
            }
            let id = format!("{}u", self.deal);
            let acc = self.acc.as_ref();
            // Решение 163: повторы заданы — сбой первой попытки известит итог повторов, а не usdt.relay_failed.
            let res = if super::usdt_return_retry::plan(&c, &self.from).is_some() {
                super::usdt_return::relay_try(&c, &id, &self.from, &self.to, acc, self.amount).await
            } else {
                super::usdt_return::relay(&c, &id, &self.from, &self.to, acc, self.amount).await
            };
            guard.done = true;
            match res {
                Ok(r) => {
                    // Решение 236: затраты возврата (ушло − зачислено + газ) — в итог сделки (usdt_return_state::set).
                    let note = format!(
                        "USDT вернулись: {} {} → {}, зачислено {}{}",
                        self.amount,
                        self.from.0,
                        self.to.0,
                        r.credited,
                        r.tail()
                    );
                    let state = r.done(self.amount, self.from, self.to);
                    st::set(&c, &self.deal, state, &note).await;
                }
                // Решение 163: исход неизвестен, вывод мог уйти, повторов нет — «не вернулись» (usdt_return_retry::fail).
                Err(e) => {
                    let way = (&self.from, &self.to);
                    if super::usdt_return_retry::fail(&c, &self.deal, self.amount, way, 0, &e).await
                    {
                        super::usdt_return_retry::schedule(&c, &self.deal, &self.from);
                    }
                }
            }
        }));
    }
}

/// Страховка на случай, если задача перегона оборвалась (паника) до итога: «не вернулись» на amount (у кусков —
/// остаток, usdt_return_pieces.rs). Ошибок нет.
pub(super) struct Guard {
    core: Core,
    deal: String,
    pub(super) amount: Decimal,
    from: VenueId,
    pub(super) done: bool,
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
