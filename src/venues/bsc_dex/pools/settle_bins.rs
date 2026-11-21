//! Файл: venues/bsc_dex/pools/settle_bins.rs
//! Что делает: решение 357 — событие пула из бинов (Applied::Bins: LB V2.1 Swap / Deposited / Withdrawn /
//! CompositionFees / FlashLoan; Infinity Bin Swap / Mint / Burn / Donate) применяется так, чтобы в ядро и в достоверное
//! состояние не попало промежуточное состояние (было: после КАЖДОГО лога Swap LB — по логу на пройденный бин — пул
//! уходил ядру с промежуточным активным бином, а бины дальше по пути — со старыми резервами; котировка завышала выход).
//! Шаги (plan → settle_read.rs read → finish):
//! 1) пул «в обновлении» ждёт активного бина A конца ЭТОГО блока, а лог его ещё не дал (активный бин памяти ≠ A) —
//!    бины лога копятся в отметке, к узлу не идём (логи одного обмена применяются вместе, без чтения на каждом);
//! 2) иначе — бины лога и накопленные бины + активный бин конца блока у узла ОДНИМ запросом на блоке события;
//! 3) активный бин памяти = активному бину узла на конце блока — пул согласован (отметка снята, достоверность — прежняя),
//!    дальше как раньше (off_map_bins.rs after, ядру); не равен — по коду контракта логи этого пула в блоке ещё идут
//!    (активный бин меняет только обмен, его последний Swap несёт конечный бин — LBPair.sol L499-L541; Infinity Bin Swap
//!    несёт activeId после обмена — BinPoolManager.sol swap, одно событие на вызов) — пул «в обновлении» (Need::Active),
//!    ядру не уходит.
//!
//! Пул с ОДНИМ логом в блоке: шаг 1 не наступает (отметки нет), шаг 2 — один запрос, как раньше (у LB раньше — тот же
//! try_many getBin; у Infinity Bin — по одному getBin), шаг 3 — активный бин памяти после единственного лога равен
//! активному бину конца блока (больше в блоке пул не менялся) — ядру сразу.
//! Кто вызывает: pools/listen_log.rs (ветка Applied::Bins); тесты (tests/bsc_dex_settle_357.rs — plan, finish).
//! От чего зависит: settle.rs (Holds, Need, active), settle_read.rs, registry.rs, listen.rs (Pools), node/call.rs.
//! Настройки: через settle_read.rs — dex.multicall3.

use std::collections::BTreeSet;

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::GatewayResult;

use super::super::node::call::block_tag;
use super::listen::Pools;
use super::registry::Registry;
use super::settle::{self, Holds, Need};
use super::settle_read::{self, Read};

/// Итог события бинов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// Пул согласован с концом блока — дальше как раньше (проверка полосы, ядру).
    Done,
    /// Пул «в обновлении» — ядру не отправлять.
    Held,
}

/// Что делать с событием (без сети).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Прочитать у узла эти бины (и активный бин) на блоке события.
    Read(Vec<u32>),
    /// Бины добавлены к отметке, к узлу не идём — пул «в обновлении».
    Held,
}

/// Шаг 1 (см. шапку). Принимает: отметки, реестр, ссылку, бины лога, блок лога. Возвращает: Plan. Ошибок нет.
pub fn plan(holds: &Holds, reg: &Registry, r: &PoolRef, ids: &[u32], block: u64) -> Plan {
    let held = holds.get(r);
    if let Some(h) = &held
        && let Some(Need::Active(a)) = h.need
        && h.block == block
        && reg.get(r).and_then(|e| settle::active(&e)) != Some(a)
    {
        holds.hold(reg, r, (block, Need::Active(a)), ids.to_vec(), false);
        return Plan::Held;
    }
    let mut all: BTreeSet<u32> = ids.iter().copied().collect();
    if let Some(h) = held {
        all.extend(h.bins);
    }
    Plan::Read(all.into_iter().collect())
}

/// Шаг 3 (см. шапку): резервы прочитанных бинов — в запись; активный бин памяти сравнивается с прочитанным.
/// Принимает: отметки, реестр, ссылку, блок, прочитанное. Возвращает: Settled. Ошибок нет.
pub fn finish(holds: &Holds, reg: &Registry, r: &PoolRef, block: u64, read: Read) -> Settled {
    let (bins, end) = read;
    reg.update(r, |e| {
        if let Some(b) = e.bins.as_mut() {
            for (id, x, y) in &bins {
                b.set(*id, *x, *y);
            }
        }
    });
    let mem = reg.get(r).and_then(|e| settle::active(&e));
    if mem.is_none_or(|a| a == end) {
        holds.release(reg, r);
        return Settled::Done;
    }
    holds.hold(reg, r, (block, Need::Active(end)), Vec::new(), true);
    Settled::Held
}

/// Событие бинов целиком: plan → чтение у узла → finish. Принимает: окружение пачки, ссылку, бины лога, блок лога.
/// Возвращает: Settled. Ошибки: узел (StaleData — решение 298), BadResponse — как раньше у перечитывания бинов.
pub async fn on_bins(p: &Pools, r: &PoolRef, ids: &[u32], block: u64) -> GatewayResult<Settled> {
    let list = match plan(&p.holds, &p.reg, r, ids, block) {
        Plan::Held => return Ok(Settled::Held),
        Plan::Read(list) => list,
    };
    // Решение 445: пул LB v2.0 — активный бин getReservesAndId (settle_read.rs read_for).
    let v20 = p
        .reg
        .get(r)
        .is_some_and(|e| matches!(&e.pool.state, PoolState::LbBin(d) if d.v20));
    match settle_read::read_for(&p.ctx, r, &list, &block_tag(block), v20).await? {
        Some(read) => Ok(finish(&p.holds, &p.reg, r, block, read)),
        None => Ok(Settled::Done),
    }
}
