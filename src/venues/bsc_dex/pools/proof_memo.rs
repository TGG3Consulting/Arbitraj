//! Файл: venues/bsc_dex/pools/proof_memo.rs
//! Что делает: решение 382, ревью 2 (Н2) — учёт доказательств «память пула неверна» (check/memory_proof.rs): по пулу
//! одновременно идёт не больше одного доказательства для одной позиции памяти (synced_block, synced_log), и уже
//! доказанная ошибка для той же позиции не доказывается снова (пул отмечен — его перечитают, позиция сменится).
//! Итоги «отставание / верна / не доказано» не запоминаются: на более поздней голове итог может стать другим — их
//! повторит следующая сверка (не чаще dex.quoter_check_s) или следующее расхождение перед сделкой.
//! Кто вызывает: exec/quoter_refetch_proof.rs, check/quoter_memory_fix.rs (через registry.rs proofs).
//! От чего зависит: std.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::core::types::PoolRef;

/// Позиция памяти пула: (synced_block, synced_log).
pub type Pos = (u64, Option<u64>);

/// Состояние учёта по пулу: позиция и доказана ли ошибка (false — доказательство идёт).
type Map = HashMap<PoolRef, (Pos, bool)>;

/// Учёт доказательств (один на реестр). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Memo {
    map: Arc<Mutex<Map>>,
}

impl Memo {
    fn lock(&self) -> std::sync::MutexGuard<'_, Map> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Начать доказательство для позиции: false — для этой позиции оно уже идёт или ошибка уже доказана. Принимает:
    /// пул, позицию. Ошибок нет.
    pub fn begin(&self, r: &PoolRef, pos: Pos) -> bool {
        let mut m = self.lock();
        if m.get(r).is_some_and(|(p, _)| *p == pos) {
            return false;
        }
        m.insert(r.clone(), (pos, false));
        true
    }

    /// Доказательство для позиции закончено: proven — ошибка доказана (запомнить), иначе — забыть (можно снова).
    /// Принимает: пул, позицию, итог. Ошибок нет.
    pub fn finish(&self, r: &PoolRef, pos: Pos, proven: bool) {
        let mut m = self.lock();
        if m.get(r).is_some_and(|(p, _)| *p == pos) {
            if proven {
                m.insert(r.clone(), (pos, true));
            } else {
                m.remove(r);
            }
        }
    }
}
