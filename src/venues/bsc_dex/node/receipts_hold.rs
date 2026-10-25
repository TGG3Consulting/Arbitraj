//! Файл: venues/bsc_dex/node/receipts_hold.rs
//! Что делает: «сброс» ленты квитанций (решение 72, ревью #62) — запрет котировок, сделок и вердиктов проверок от
//! полного сброса или остановки ленты до момента, когда все пачки старой ленты сделали свои пулы недостоверными
//! (Invalidated) и закрыли «соединения» (pools/session.rs): reset — сброс, stop_source — остановка из-за смены
//! источника (отставание обнуляется, иначе при ws котировок не будет никогда), release_if_settled — снятие, old_links —
//! какие пачки ещё держат.
//! Кто вызывает: node/receipts_task.rs, node/receipts_handle.rs (снятие последним «соединением» остановленной ленты).
//! От чего зависит: receipts.rs (Feed, State), events_lag.rs.
//! Настройки: нет.

use std::sync::atomic::Ordering;

use super::receipts::{Feed, State};

impl State {
    /// Имена пачек, чьи «соединения» прошлых поколений ещё открыты. Ошибок нет.
    pub(super) fn old_links(&self) -> Vec<String> {
        let g = self.generation;
        self.links
            .values()
            .filter(|(lg, _)| *lg < g)
            .map(|(_, n)| {
                if n.is_empty() {
                    "без имени".to_string()
                } else {
                    n.clone()
                }
            })
            .collect()
    }
}

impl Feed {
    /// Полный сброс ленты (решение 72, ревью #62): «сброс» на отметке отставания стоит (котировки, сделки и вердикты
    /// проверок запрещены), отставание не снимается, затем обрыв. Ошибок нет.
    pub fn reset(&self, why: &str) {
        self.inner.lag.hold();
        self.cut(why);
    }

    /// Снимает «сброс», когда все «соединения» прошлых поколений закрыты (их пачки уже сделали пулы недостоверными).
    /// Циклы не будит. Возвращает: true — «сброс» снят сейчас. Ошибок нет.
    pub fn release_if_settled(&self) -> bool {
        self.inner.lag.held() && self.lock().old_links().is_empty() && self.inner.lag.release()
    }

    /// Остановка ленты из-за смены источника (ревью #62): отметка одна на площадку и её снимает только лента — снять
    /// здесь, иначе при ws котировок не будет никогда. Отставание обнуляется без пробуждения циклов, «сброс» держится,
    /// пока пачки ленты не закрыли «соединения» — а закрывают они их после Invalidated своих пулов (pools/session.rs):
    /// по старым данным не котируем ни мгновения; снимает его последнее закрытое «соединение» (receipts_handle.rs),
    /// раз задачи ленты больше нет. Ошибок нет.
    pub fn stop_source(&self, why: &str) {
        self.inner.lag.hold();
        self.inner.lag.set(0);
        self.inner.source_off.store(true, Ordering::SeqCst);
        // Сначала «задача кончилась», потом обрыв: «соединение», открытое после обрыва, запустит ленту заново.
        self.stopped();
        self.cut(why);
        self.release_if_settled();
    }

    /// Пачки, чьи «соединения» старой ленты ещё открыты и держат «сброс» (для журнала). Ошибок нет.
    pub fn holders(&self) -> Vec<String> {
        self.lock().old_links()
    }

    /// Лента остановлена сменой источника и новой не запущено. Ошибок нет.
    pub(super) fn source_off(&self) -> bool {
        self.inner.source_off.load(Ordering::SeqCst)
    }
}
