//! Файл: venues/bsc_dex/pools/prefetch_memo.rs
//! Что делает: решение 325 — какие view-вызовы сделал код чтения пула при ПРОШЛОМ перечитывании по событию (книга
//! node/prefetch.rs, used): при следующем перечитывании этого пула эти же вызовы читаются заранее одним обращением к
//! узлу на новом блоке (pools/prefetch_read.rs). Это подсказка для любого вида пула без своего кода: у вида с
//! неизменным набором вызовов (DODO, Solidly, StableSwap, Balancer, LB, Smardex…) со второго перечитывания весь набор
//! — одно обращение; у вида с цепочкой (обход бинов, списка тиков) — всё, что не изменилось с прошлого раза.
//! Хранится в памяти процесса по (площадка, ссылка на пул); последнее чтение заменяет прежнее. Неверная подсказка
//! итог не меняет: вызов, которого нет в книге, идёт к узлу как раньше.
//! Кто вызывает: pools/prefetch_read.rs.
//! От чего зависит: node/prefetch.rs (Call), core/types (PoolRef, VenueId).
//! Настроек нет (включает вызывающий — dex.event_refresh_batch).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::core::types::{PoolRef, VenueId};

use super::super::node::prefetch::Call;

/// Вызовы прошлого чтения по (площадка, пул).
type Memo = HashMap<(VenueId, PoolRef), Vec<Call>>;

/// Вызовы прошлого чтения по пулам.
static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Вызовы прошлого чтения пула (нет — пусто). Принимает: площадку, ссылку на пул. Ошибок нет.
pub fn get(venue: &VenueId, r: &PoolRef) -> Vec<Call> {
    MEMO.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(venue.clone(), r.clone()))
        .cloned()
        .unwrap_or_default()
}

/// Запоминает вызовы чтения пула (заменяет прежние). Пустой список — запись удаляется. Ошибок нет.
pub fn put(venue: &VenueId, r: &PoolRef, calls: Vec<Call>) {
    let mut g = MEMO.lock().unwrap_or_else(|p| p.into_inner());
    let key = (venue.clone(), r.clone());
    if calls.is_empty() {
        g.remove(&key);
    } else {
        g.insert(key, calls);
    }
}
