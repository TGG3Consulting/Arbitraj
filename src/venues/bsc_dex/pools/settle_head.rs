//! Файл: venues/bsc_dex/pools/settle_head.rs
//! Что делает: решение 357, отметка head — ревью слияния «357 × 339» (Б1, деньги): чтение Liquidity Book V2.1 на голове
//! (per_block_lb.rs, решение 290) не прочиталось — пул недостоверен и помечен «нужно полное чтение». Следующие головы
//! (после паузы решения 339, если она есть — per_block.rs пул на паузе не читает) читают такой пул ЦЕЛИКОМ
//! (reload::load_one_block — состояние и ВСЕ бины полосы), удачное чтение — registry.rs put достоверной записи — снимает
//! отметку. Голова LB (lb_head.rs) достоверность НЕ поднимает никогда: её сравнение «равны» видит только активный бин и
//! его резервы, а события, пропущенные без перечитывания бинов (пауза 339: listen_log.rs `Bins if waiting`; StaleData,
//! решение 298; обрыв сессии), меняют и неактивные бины (вывод — в памяти лишняя ликвидность, котировка завышала бы
//! выход). Было (t154 до ревью слияния): удачная голова возвращала was_valid отметки (head_ok) — путь паузы 339 и
//! переживание полного чтения давали valid = true с неперечитанными бинами.
//! Кто вызывает: pools/per_block_lb.rs (head_failed, needs_full).
//! От чего зависит: settle.rs (Holds, Hold), registry.rs.
//! Настройки: нет.

use std::collections::BTreeSet;

use crate::core::types::PoolRef;

use super::registry::Registry;
use super::settle::{Hold, Holds};

impl Holds {
    /// Чтение LB на голове не прочиталось (per_block_lb.rs): пул недостоверен и «нужно полное чтение» (отметка head; у
    /// пула с отметкой «в обновлении» — к ней). Ошибок нет.
    pub fn head_failed(&self, reg: &Registry, r: &PoolRef) {
        let Some(was) = reg.update(r, |e| std::mem::replace(&mut e.valid, false)) else {
            return;
        };
        self.with(|m| {
            if let Some(h) = m.get_mut(r) {
                h.head = true;
            } else {
                let h = Hold {
                    block: 0,
                    was_valid: was,
                    bins: BTreeSet::new(),
                    need: None,
                    head: true,
                    tx: None,
                };
                m.insert(r.clone(), h);
            }
        });
    }

    /// Нужно ли пулу полное чтение вместо чтения головы (отметка head). Снимает её только полное чтение (registry.rs
    /// put достоверной записи). Ошибок нет.
    pub fn needs_full(&self, r: &PoolRef) -> bool {
        self.get(r).is_some_and(|h| h.head)
    }
}
