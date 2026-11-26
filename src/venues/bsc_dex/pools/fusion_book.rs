//! Файл: venues/bsc_dex/pools/fusion_book.rs
//! Что делает: решение 463 (T463-02; ревью t236) — книга данных оракула пулов THENA Fusion (вынесено из
//! fusion_head.rs: файл вырос): последние данные, прочитанные на блоке (remember из fusion_fee.rs next_fee), по ним
//! fusion_head.rs пересчитывает комиссию следующего блока на каждой голове без узла.
//! Книга (ревью t236, п. 4): более старый блок не вытесняет более новый, тот же номер — заменяет (после реорга это
//! канонический блок); снятые пулы — forget (registry.rs retain); лог пула с removed (реорг) — orphan: данные,
//! прочитанные на блоке лога или позже, долой (могли быть с сиротского блока; сравнение номера блока).
//! Сверка с квотером на блоке (решение 73): own_read — чтение сверки пишет данные в свою ячейку задачи (tokio
//! task_local), а не в книгу; same_block — комиссия свопа в САМОМ блоке H по данным оракула этого же чтения.
//! Слияние BSC → Arbitrum (t256, как решение 435 у registry_key.rs): ключ книги — (площадка DEX, пул): пул с тем же
//! адресом в другой сети — другой контракт (THENA Fusion — BSC, Zyberswap V3 — Arbitrum). forget — по ссылке у всех
//! площадок (как prefetch_memo.rs: лишнее забывание ведёт лишь к полному чтению).
//! Повторное ревью t236 (п. 3): чтения, не пишущие реестр, — только через own_read / isolated (сверка, перечитывание
//! перед сделкой, доказательство памяти): в книгу попадают лишь данные чтений, чья запись идёт в реестр.
//! Кто вызывает: pools/fusion_fee.rs (remember), pools/fusion_head.rs (with), check/quoter_block.rs (own_read,
//! same_block), exec/quoter_refetch.rs, check/memory_proof.rs, quote/verify.rs, quote/cycle_legs.rs,
//! check/hook_batch.rs (isolated), registry.rs (forget), listen_log.rs (orphan).
//! От чего зависит: fusion_oracle.rs (Got), fusion_fee.rs (gaps), math/fee_algebra_v1.rs (fee_at), registry.rs
//! (Entry), families.rs (Proto).
//! Настройки: нет.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{LazyLock, Mutex, MutexGuard};

use alloy_primitives::Address;

use crate::core::types::{PoolRef, PoolState};

use super::super::math::fee_algebra_v1;
use super::families::Proto;
use super::fusion_oracle::Got;
use super::registry::Entry;

/// Данные оракула пула, прочитанные на блоке `block`.
#[derive(Debug, Clone)]
pub struct Held {
    pub block: u64,
    pub got: Got,
}

/// Ключ книги: (площадка DEX — Ctx.place.id, адрес пула).
type Key = (&'static str, Address);

/// Последние прочитанные данные оракула по пулу THENA Fusion.
static BOOK: LazyLock<Mutex<HashMap<Key, Held>>> = LazyLock::new(Default::default);

tokio::task_local! {
    /// Ячейка чтения сверки (own_read): задана — remember пишет сюда, а не в книгу.
    static OWN: RefCell<Option<Held>>;
}

/// Книга под замком (отравленный замок — данные как есть). Ошибок нет.
fn book() -> MutexGuard<'static, HashMap<Key, Held>> {
    BOOK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Запоминает данные оракула пула, прочитанные на блоке `block` (в ячейку own_read, если задача в ней; иначе в книгу:
/// более старый блок не вытесняет более новый, тот же номер — заменяет: после реорга это канонический блок).
/// Принимает: площадку DEX, пул, блок, данные. Ошибок нет.
pub fn remember(venue: &'static str, pool: Address, block: u64, got: Got) {
    let key = (venue, pool);
    let mut held = Some(Held { block, got });
    if OWN.try_with(|c| *c.borrow_mut() = held.take()).is_ok() {
        return;
    }
    let Some(held) = held else {
        return;
    };
    let mut m = book();
    if m.get(&key).is_some_and(|h| h.block > block) {
        return;
    }
    m.insert(key, held);
}

/// Чтение `f` с ячейкой own_read: данные оракула, прочитанные внутри (fusion_fee.rs next_fee), — в итог, не в книгу.
/// Принимает: future чтения. Возвращает: (итог чтения, данные оракула; None — не читались). Ошибок нет.
pub async fn own_read<F: Future>(f: F) -> (F::Output, Option<Held>) {
    OWN.scope(RefCell::new(None), async {
        let out = f.await;
        (out, OWN.with(|c| c.borrow_mut().take()))
    })
    .await
}

/// Повторное ревью t236 (п. 3): чтение `f`, не пишущее реестр (перечитывание перед сделкой, проверки), —
/// данные оракула THENA Fusion внутри него не попадают в книгу (иначе книга оказалась бы новее записи реестра).
/// Принимает: future чтения. Возвращает: его итог. Ошибок нет.
pub async fn isolated<F: Future>(f: F) -> F::Output {
    own_read(f).await.0
}

/// Сверка с квотером на блоке (решение 73): в свежем состоянии пула, прочитанном на `block`, комиссия — та, что получит
/// своп в САМОМ этом блоке (fee_at на время блока чтения), по данным оракула этого же чтения `own`. Не THENA Fusion —
/// ничего. Принимает: запись, данные чтения, блок. Ошибки (текст): данных чтения нет / не того блока, комиссия не
/// считается.
pub fn same_block(e: &mut Entry, own: Option<&Held>, block: u64) -> Result<(), String> {
    if e.family.proto != Proto::Algebra {
        return Ok(());
    }
    let PoolState::Algebra { tick, fee, .. } = &mut e.pool.state else {
        return Ok(());
    };
    let Some(h) = own.filter(|h| h.block == block) else {
        return Err(format!(
            "THENA Fusion: данных оракула чтения на блоке {block} нет"
        ));
    };
    let o = &h.got.oracle;
    if h.got.ts < o.last.ts {
        return Err(format!(
            "THENA Fusion: время блока {block} раньше последней точки оракула"
        ));
    }
    match fee_algebra_v1::fee_at(o, h.got.ts, *tick) {
        Some(f) => {
            *fee = f;
            Ok(())
        }
        None => Err(format!(
            "THENA Fusion: комиссия блока {block} не считается{}",
            super::fusion_fee::gaps(&h.got.gaps)
        )),
    }
}

/// Пулы `gone` сняты из реестра — их данные долой (только у переданной площадки). Ошибок нет.
pub fn forget(venue: &str, gone: &[PoolRef]) {
    let gone: std::collections::HashSet<Address> = gone
        .iter()
        .filter_map(|r| match r {
            PoolRef::Contract(a) => Some(*a),
            _ => None,
        })
        .collect();
    if gone.is_empty() {
        return;
    }
    book().retain(|(v, a), _| *v != venue || !gone.contains(a));
}

/// Лог пула с removed (реорг) на блоке `block`: данные, прочитанные на нём или позже, долой. Принимает: площадку
/// DEX, ссылку, блок. Ошибок нет.
pub fn orphan(venue: &'static str, r: &PoolRef, block: u64) {
    let PoolRef::Contract(a) = r else {
        return;
    };
    let key = (venue, *a);
    let mut m = book();
    if m.get(&key).is_some_and(|h| h.block >= block) {
        m.remove(&key);
    }
}

/// Данные книги по пулу — в функцию `f` (под замком книги; .await внутри нет). Принимает: площадку DEX, адрес пула,
/// функцию. Возвращает: её итог. Ошибок нет.
pub fn with<T>(venue: &'static str, pool: Address, f: impl FnOnce(Option<&Held>) -> T) -> T {
    f(book().get(&(venue, pool)))
}
