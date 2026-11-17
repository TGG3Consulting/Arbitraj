//! Файл: venues/bsc_dex/pools/proof_memo.rs
//! Что делает: решение 382, ревью 2 (Н2) и ревью 3 (Н3) — учёт доказательств «память пула неверна»
//! (check/memory_proof.rs): по пулу одновременно идёт не больше одного доказательства для одной позиции памяти
//! (synced_block, synced_log), уже доказанная ошибка для той же позиции не доказывается снова (пул отмечен — его
//! перечитают, позиция сменится). Итоги «отставание / верна / не доказано» не запоминаются: на более поздней голове
//! итог может стать другим. Учёт держит охранник (Guard): его Drop закрывает запись всегда — и при панике, и при отмене
//! задачи, — иначе позиция тихого пула осталась бы «идёт» навсегда. Записи пулов, которых больше нет среди найденных,
//! чистит keep_only (pools/market.rs, как отметки неудач и «в обновлении»).
//! Кто вызывает: exec/quoter_refetch_proof.rs, check/quoter_memory_fix.rs (через registry.rs proofs), pools/market.rs.
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

/// Охранник одного доказательства: Drop закрывает запись (proven — ошибка доказана и пул отмечен).
pub struct Guard {
    memo: Memo,
    r: PoolRef,
    pos: Pos,
    proven: bool,
}

impl Guard {
    /// Итог: ошибка доказана и пул отмечен (запомнить для этой позиции). Ошибок нет.
    pub fn proven(&mut self, v: bool) {
        self.proven = v;
    }
}

impl Drop for Guard {
    /// Закрыть запись учёта (и при панике или отмене задачи — тогда как «не доказано»). Ошибок нет.
    fn drop(&mut self) {
        self.memo.finish(&self.r, self.pos, self.proven);
    }
}

impl Memo {
    fn lock(&self) -> std::sync::MutexGuard<'_, Map> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Начать доказательство для позиции. Принимает: пул, позицию. Возвращает: охранника; None — для этой позиции
    /// доказательство уже идёт или ошибка уже доказана. Ошибок нет.
    pub fn begin(&self, r: &PoolRef, pos: Pos) -> Option<Guard> {
        let mut m = self.lock();
        if m.get(r).is_some_and(|(p, _)| *p == pos) {
            return None;
        }
        m.insert(r.clone(), (pos, false));
        Some(Guard {
            memo: self.clone(),
            r: r.clone(),
            pos,
            proven: false,
        })
    }

    /// Доказательство для позиции закончено: proven — запомнить, иначе — забыть (можно снова). Позиция пула уже
    /// другая (начато новое) — ничего. Ошибок нет.
    fn finish(&self, r: &PoolRef, pos: Pos, proven: bool) {
        let mut m = self.lock();
        if m.get(r).is_some_and(|(p, _)| *p == pos) {
            if proven {
                m.insert(r.clone(), (pos, true));
            } else {
                m.remove(r);
            }
        }
    }

    /// Оставить записи только пулов, для которых `keep` — да (пулы, которых больше нет, — долой). Ошибок нет.
    pub fn keep_only(&self, keep: impl Fn(&PoolRef) -> bool) {
        self.lock().retain(|r, _| keep(r));
    }
}
