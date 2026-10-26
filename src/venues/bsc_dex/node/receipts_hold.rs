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
            .filter(|l| l.generation < g)
            .map(|l| {
                if l.name.is_empty() {
                    "без имени".to_string()
                } else {
                    l.name.clone()
                }
            })
            .collect()
    }

    /// Пачки, чьи «соединения» ещё не перечитали пулы до текущей эпохи пересинхрона (решение 77). Ошибок нет.
    pub(super) fn unsynced(&self) -> Vec<String> {
        let e = self.resync_epoch;
        self.links
            .values()
            .filter(|l| l.resynced < e)
            .map(|l| {
                if l.name.is_empty() {
                    "без имени".to_string()
                } else {
                    l.name.clone()
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

    /// Пропуск с пересинхроном (решение 77, вариант B): «сброс» на отметке отставания стоит (котировки, сделки и
    /// вердикты запрещены — не торгуем вслепую), подписки НЕ рвутся, поколение НЕ меняется; эпоха пересинхрона растёт
    /// и будит пачки перечитать состояние пулов (pools/session.rs). Снимается, когда все «соединения» перечитали
    /// (release_if_resynced). Ошибок нет.
    pub fn request_resync(&self) {
        self.inner.lag.hold();
        let epoch = {
            let mut s = self.lock();
            s.resync_epoch += 1;
            s.resync_epoch
        };
        let _ = self.inner.resync.send(epoch);
    }

    /// Снимает «сброс» пересинхрона, когда все живые «соединения» перечитали пулы до текущей эпохи. Циклы не будит.
    /// Возвращает: true — снят сейчас. Ошибок нет.
    pub fn release_if_resynced(&self) -> bool {
        self.inner.lag.held() && self.lock().unsynced().is_empty() && self.inner.lag.release()
    }

    /// Приёмник эпохи пересинхрона для пачки (pools/session.rs). Ошибок нет.
    pub fn resync_watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.resync.subscribe()
    }

    /// Пачки, чьи «соединения» старой ленты ещё открыты и держат «сброс» (для журнала). Ошибок нет.
    pub fn holders(&self) -> Vec<String> {
        let s = self.lock();
        let mut v = s.old_links();
        v.extend(s.unsynced());
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Лента остановлена сменой источника и новой не запущено. Ошибок нет.
    pub(super) fn source_off(&self) -> bool {
        self.inner.source_off.load(Ordering::SeqCst)
    }
}
