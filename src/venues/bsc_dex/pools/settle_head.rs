//! Файл: venues/bsc_dex/pools/settle_head.rs
//! Что делает: решение 357, отметка head (ревью 332, п. 2 «найдено, не исправлено»): чтение Liquidity Book V2.1 на
//! голове (per_block_lb.rs, решение 290) не прочиталось — пул недостоверен (как раньше), а достоверность ему возвращает
//! только удачное чтение головы и только ту, что была до неудачи (lb_head.rs put → head_ok). Раньше удачная голова
//! (load::renewed) ставила valid = true ЛЮБОЙ записи LB, в т. ч. пулу, ждущему полного перечитывания после StaleData
//! (решение 298): его бины события не перечитаны (например, вывод из неактивного бина — в памяти лишняя ликвидность),
//! а сравнение головы смотрит только активный бин и его резервы — «равны», пул достоверен с неполными бинами, котировка
//! могла завысить выход. Теперь голова valid не поднимает (lb_head.rs renewed), кроме снятия своей отметки.
//! Кто вызывает: pools/per_block_lb.rs (head_failed), pools/lb_head.rs (head_ok).
//! От чего зависит: settle.rs (Holds, Hold), registry.rs.
//! Настройки: нет.

use std::collections::BTreeSet;

use crate::core::types::PoolRef;

use super::registry::Registry;
use super::settle::{Hold, Holds};

impl Holds {
    /// Чтение LB на голове не прочиталось (per_block_lb.rs): пул недостоверен. Был достоверен — отметка head (вернёт
    /// удачная голова, head_ok); уже «в обновлении» — к отметке добавляется head; недостоверен по другой причине —
    /// только valid = false, как раньше (его ведёт свой путь). Ошибок нет.
    pub fn head_failed(&self, reg: &Registry, r: &PoolRef) {
        let Some(was) = reg.update(r, |e| std::mem::replace(&mut e.valid, false)) else {
            return;
        };
        self.with(|m| {
            if let Some(h) = m.get_mut(r) {
                h.head = true;
            } else if was {
                let h = Hold {
                    block: 0,
                    was_valid: true,
                    bins: BTreeSet::new(),
                    need: None,
                    head: true,
                };
                m.insert(r.clone(), h);
            }
        });
    }

    /// Чтение LB на голове прочиталось и согласовано с памятью (lb_head.rs put): снимает отметку head. Возвращает:
    /// Some(достоверность до отметки) — её вернуть записи (отметка удалена: пул не «в обновлении»); None — вернуть
    /// нечего (отметки head не было или пул ещё «в обновлении»). Ошибок нет.
    pub fn head_ok(&self, r: &PoolRef) -> Option<bool> {
        if !self.any() {
            return None;
        }
        self.with(|m| match m.get(r).map(|h| h.head && h.need.is_none()) {
            Some(true) => m.remove(r).map(|h| h.was_valid),
            Some(false) => {
                if let Some(h) = m.get_mut(r) {
                    h.head = false;
                }
                None
            }
            None => None,
        })
    }
}
