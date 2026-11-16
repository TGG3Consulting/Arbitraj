//! Файл: venues/bsc_dex/pools/lb_head.rs
//! Что делает: ревью решения 332 (Б1, деньги) — чтение Liquidity Book V2.1 «каждым блоком» (per_block_lb.rs, решение 290)
//! больше НИКОГДА не кладёт в память «новый активный бин + старые резервы бинов» и не глотает события блока.
//! Было: на голове B — getActiveId и комиссия на B, бины прежние, блок B отмечен учтённым (load::renewed) → логи
//! Swap / Deposited / Withdrawn блока B, обработанные позже (режим квитанций — всегда позже головы; WebSocket — порядок
//! select! случайный), отбрасывались (registry.rs has_applied): активный бин сдвинут, резервы пройденных бинов и нового
//! активного устарели — котировка могла ЗАВЫСИТЬ выход; перечитывания по решению 332 не было.
//! Стало (правило): чтение на голове пишет в память ТОЛЬКО комиссию и время блока, и только когда на блоке чтения R
//! активный бин равен активному бину памяти И резервы этого бина (getBin в той же пачке) равны резервам памяти (same).
//! Активный бин и бины чтение на голове не трогает никогда; отметку учтённого (synced_block / synced_log) не двигает —
//! события блоков применяются (listen_log.rs: бины перечитываются на блоке события, затем off_map_bins.rs).
//! Почему «равны» достаточно: любой обмен LB либо меняет резервы активного бина (вход ложится в него), либо сдвигает
//! активный бин (бин пуст в сторону выхода — LBPair.sol L499-L550 swap: цикл с activeId, _getNextNonEmptyBin); займ
//! (FlashLoan) и CompositionFees кладут в активный бин. Параметры комиссии меняют обмен и ввод в активный бин с комиссией
//! состава (LBPair.sol L989-L1008 — резервы активного бина при этом тоже меняются); StaticFeeParametersSet / ForcedDecay
//! ведут к полному перечитыванию (Refresh, events_lb.rs). Значит, при «равны» ждущие события до R — только
//! ввод / вывод ликвидности в другие бины: комиссии и активного бина они не меняют, а сами применятся, когда придут.
//! Не равны (ждут события обмена до R) — на R ничего не пишется; если память отстаёт (synced_block S < R), то же
//! сравнение на S (at_synced): равны — комиссия и время блока S (снимок S, согласованный с памятью); нет — ничего,
//! события придут и применятся, комиссия обновится на следующей голове с «равны».
//! Оба порядка: (1) логи блока B раньше головы B — память на конец B (бины перечитаны на B), голова: равны → комиссия B;
//! (2) голова B раньше логов B — обмен в B → не равны → память не трогается; логи B не отброшены (отметка не двигалась),
//! применяются, бины перечитываются на B, off_map_bins проверяет полосу. Голова B+1 раньше логов B — то же, что (2).
//! Активный бин вне прочитанной полосы — резервов памяти нет, «равны» не проверить — не равны (ничего не пишется).
//! Это исправление ошибки с риском денег — НЕ под настройкой (DECISIONS «332 (ревью)»).
//! Решение 357 (п. 2 «найдено» ревью 332): чтение на голове НЕ поднимает достоверность (renewed — valid записи как
//! был): раньше load::renewed ставил valid = true и пулу, ждущему полного перечитывания после StaleData (решение 298) с
//! неперечитанными бинами события (вывод из неактивного бина — лишняя ликвидность в памяти, «равны» на голове её не
//! видит). Ревью слияния «357 × 339» (Б1): голова не возвращает достоверность и тому, у кого сняла её сама, — такой
//! пул per_block_lb.rs читает целиком (settle_head.rs needs_full); недостоверная запись ядру не отправляется.
//! Кто вызывает: pools/per_block_lb.rs (calls, decode, same, renewed, at_synced, put).
//! От чего зависит: load_lb.rs (calls, decode, time_call, decode_time), load.rs (renewed, Loaded), abi_lb.rs (getBin),
//! node/call.rs, node/multicall.rs, load_batch.rs, load_smardex.rs (block_secs), listen_log.rs, registry.rs,
//! reload.rs (put_new — задача t160, dex.per_block_skip_same).
//! Настройки: через вызываемые (dex.load_batch, dex.multicall3 [bsc_dex]).
//!
//! Источник: lfj-gg/joe-v2@v2.1.0 — src/LBPair.sol L181-L183 getBin, L171-L173 getActiveId, L499-L550 swap, L603-L613
//! flashLoan (tests/data/lb_v21_2026-11-15/source_excerpt.txt).

use std::collections::BTreeMap;

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes};

use crate::core::types::{LbBinData, PoolState};

use super::super::node::{call, multicall};
use super::abi_lb::ILBPair as P;
use super::discover::Found;
use super::listen::{End, Pools};
use super::load::{self, Loaded};
use super::registry::Entry;
use super::{listen_log, load_batch, load_lb, load_smardex, reload};

/// Вызовов на пул в пачке головы: состояние (load_lb::calls) и getBin(активный бин памяти).
pub const PER_POOL: usize = load_lb::PER_POOL + 1;

/// Активный бин записи LB (None — запись не LB). Ошибок нет.
pub fn active_of(e: &Entry) -> Option<u32> {
    match &e.pool.state {
        PoolState::LbBin(d) => Some(d.active_id),
        _ => None,
    }
}

/// Вызовы пула на голове: getActiveId, getStaticFeeParameters, getVariableFeeParameters, getBin(`active`).
/// Принимает: адрес пула, активный бин памяти. Ошибок нет.
pub fn calls(pool: Address, active: u32) -> Vec<(Address, Bytes)> {
    let mut out = load_lb::calls(pool);
    let c = P::getBinCall {
        id: U24::from(active),
    };
    out.push((pool, Bytes::from(c.abi_encode())));
    out
}

/// Разбор ответов calls (без сети; её же зовут тесты на записанных ответах). Принимает: ответы по порядку, состояние
/// памяти (порядок X/Y, шаг), время блока чтения. Возвращает: (состояние на блоке чтения, резервы бина (X, Y)).
/// Ошибка — ответов не PER_POOL, ответ не по ABI.
pub fn decode(a: &[Bytes], mem: &LbBinData, ts: u64) -> Result<(LbBinData, (u128, u128)), String> {
    if a.len() != PER_POOL {
        return Err(format!("ответов {} из {PER_POOL}", a.len()));
    }
    let d = load_lb::decode(&a[..load_lb::PER_POOL], mem.x_is_token0, mem.bin_step, ts)?;
    let b = P::getBinCall::abi_decode_returns(&a[load_lb::PER_POOL])
        .map_err(|e| format!("getBin: {e}"))?;
    Ok((d, (b.binReserveX, b.binReserveY)))
}

/// Согласовано ли чтение с памятью (см. шапку): активный бин тот же, он в прочитанной полосе, его резервы те же
/// (пустой бин в карте не хранится — (0, 0)). Принимает: запись, прочитанное состояние, резервы бина. Ошибок нет.
pub fn same(e: &Entry, read: &LbBinData, bin: (u128, u128)) -> bool {
    let (Some(active), Some(bins)) = (active_of(e), e.bins.as_ref()) else {
        return false;
    };
    if active != read.active_id || !bins.covers(active) {
        return false;
    }
    bins.reserves.get(&active).copied().unwrap_or((0, 0)) == bin
}

/// Запись с комиссией и временем блока чтения (активный бин и бины — памяти, они равны прочитанному по same) на блоке
/// `block`; отметка учтённого (synced_block, synced_log) — прежняя: события не отбрасываются; достоверность — прежняя
/// (решение 357: голова её не поднимает — только put по отметке head). Ошибок нет.
pub fn renewed(e: &Entry, block: u64, d: LbBinData) -> Entry {
    let loaded = Loaded {
        state: PoolState::LbBin(Box::new(d)),
        ticks: None,
        bins: e.bins.clone(),
        fee_pips: None,
    };
    let mut out = load::renewed(e, block, loaded);
    out.synced_block = e.synced_block;
    out.synced_log = e.synced_log;
    out.valid = e.valid;
    out
}

/// Запись в реестр и ядру — как у per_block_lb.rs. Решение 357 (ревью слияния, Б1): голова достоверность не поднимает
/// никогда (renewed — как была; пул с отметкой «нужно полное чтение» per_block_lb.rs читает целиком); недостоверная
/// запись ядру не уходит.
/// Задача t160 (слияние с t153): `skip` (reload::skip_same, dex.per_block_skip_same) — запись через reload::put_new:
/// поколение состояния не сменилось — ядру не шлётся (как reload::put_emit у прочих видов); false — как раньше.
/// Ошибки: End — ядро закрыло канал.
pub async fn put(p: &Pools, f: &Found, e: Entry, skip: bool) -> Result<(), End> {
    listen_log::note_refill(p, &e);
    let (pool, valid) = (e.pool.clone(), e.valid);
    let fresh = reload::put_new(p, e, skip);
    p.loading.done(&f.reference);
    p.loading.publish(p).await;
    if !valid || !fresh {
        return Ok(());
    }
    listen_log::emit(p, pool)
}

/// Пулы, у которых на голове `head` чтение не согласовано с памятью: память отстаёт (synced_block S < head) — то же
/// чтение и сравнение на S (пачка на каждый S); равны — комиссия и время блока S. Сбой чтения или «не равны» — ничего
/// (память не трогается, valid не меняется: события придут и применятся). Принимает: пулы, голову, skip (как у put).
/// Ошибки: End — ядро закрыло канал.
pub async fn at_synced(p: &Pools, pools: &[&Found], head: u64, skip: bool) -> Result<(), End> {
    let mut by_block: BTreeMap<u64, Vec<(&Found, u32)>> = BTreeMap::new();
    for f in pools.iter().copied() {
        let Some(e) = p.reg.get(&f.reference) else {
            continue;
        };
        if let Some(a) = active_of(&e)
            && e.synced_block < head
        {
            by_block.entry(e.synced_block).or_default().push((f, a));
        }
    }
    for (block, list) in by_block {
        let mut all: Vec<(Address, Bytes)> = list
            .iter()
            .flat_map(|(f, a)| calls(f.reference.address(), *a))
            .collect();
        let mc = multicall::address(&p.ctx);
        if let Some(m) = mc {
            all.push(load_lb::time_call(m));
        }
        let size = load_batch::batch(&p.ctx).unwrap_or(all.len()).max(1);
        let got = call::try_chunks(&p.ctx, &all, &call::block_tag(block), size).await;
        let ts = match mc {
            Some(_) => match got.last() {
                Some(Ok(Ok(b))) => load_lb::decode_time(b).ok(),
                _ => None,
            },
            None => load_smardex::block_secs(&p.ctx, block).await.ok(),
        };
        let Some(ts) = ts else {
            continue;
        };
        for ((f, _), part) in list.iter().zip(got.chunks(PER_POOL)) {
            let Some(e) = p.reg.get(&f.reference) else {
                continue;
            };
            let answers: Result<Vec<Bytes>, String> =
                part.iter().map(|x| x.clone().and_then(|r| r)).collect();
            let read = match (answers, &e.pool.state) {
                (Ok(a), PoolState::LbBin(d)) => decode(&a, d, ts).ok(),
                _ => None,
            };
            // Память могла сдвинуться (события применены) — сравнение с текущей записью и её отметкой.
            if let Some((d, bin)) = read
                && e.synced_block == block
                && same(&e, &d, bin)
            {
                put(p, f, renewed(&e, block, d), skip).await?;
            }
        }
    }
    Ok(())
}
