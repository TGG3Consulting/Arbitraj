//! Файл: venues/bsc_dex/pools/settle_reread.rs
//! Что делает: решение 382 — «память пула доказанно разошлась с узлом»: пул недостоверен (котировки, граф, проверки и
//! сделки его не берут, ядру не уходит) и ждёт ПОЛНОГО чтения своей сессией пачки — отметка settle.rs Need::Reread:
//! её закрывает новая голова выше блока отметки (settle_close.rs: reload::load_one — состояние и карта тиков/бинов
//! целиком) или первый же лог пула (settle_lost.rs lost → очередь повтора пачки). Достоверность возвращает только
//! полное чтение (registry.rs put достоверной записи снимает отметку). Кто ставит: перечитывание перед сделкой
//! (exec/quoter_refetch.rs — память не равна узлу на блоке H) и «память отстаёт от сети» периодической сверки
//! (check/quoter_one.rs). Пул уже недостоверный или спящий (мелкий, не котируется) не отмечается — его ведёт свой путь.
//! Кто вызывает: exec/quoter_refetch.rs, check/quoter_one.rs.
//! От чего зависит: settle.rs (Holds, Need), registry.rs.
//! Настройки: нет (базовое поведение; предел — блоки сети).

use crate::core::types::PoolRef;

use super::registry::Registry;
use super::settle::{Holds, Need};

impl Holds {
    /// Поставить пул на полное чтение (см. шапку). Принимает: реестр, пул, блок, на котором доказано расхождение.
    /// Возвращает: true — отметка поставлена; false — пула нет в реестре, он недостоверен или спит. Ошибок нет.
    pub fn reread(&self, reg: &Registry, r: &PoolRef, block: u64) -> bool {
        let Some(e) = reg.entry(r) else {
            return false;
        };
        if !e.valid || e.asleep {
            return false;
        }
        self.hold(reg, r, (block, Need::Reread), Vec::new(), false);
        true
    }

    /// Ждёт ли пул полного чтения по решению 382. Ошибок нет.
    pub fn rereading(&self, r: &PoolRef) -> bool {
        self.get(r).is_some_and(|h| h.need == Some(Need::Reread))
    }
}
