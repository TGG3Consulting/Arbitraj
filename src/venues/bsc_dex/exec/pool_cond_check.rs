//! Файл: venues/bsc_dex/exec/pool_cond_check.rs
//! Что делает: ревью 438 (C) — самопроверка слотов условия пулов на блоке плана. Слоты выбраны по виду DEX из
//! настроек (exec/pool_cond.rs), а не по коду контракта: DEX с другой раскладкой под видом uniswap_v2/v3/pancake дал
//! бы условие, которое молча не работает или даёт ложные отказы. Поэтому прочитанные слова разбираются по раскладке и
//! сверяются с состоянием того же пула, перечитанным на том же блоке (quote/cycle_legs.rs fresh_at): V2 — reserve0/1;
//! V3, PancakeSwap V3, Slipstream, Uniswap V4 — sqrtPriceX96, tick, liquidity; Kyber Elastic — sqrtP, currentTick,
//! baseL; LB 2.1 — activeId. Расхождение (или вид и состояние не сочетаются) — пул своим контрактом получает корень
//! хранилища (одна пачка eth_getProof на блоке плана, только при расхождении; срок verify.timeout_ms), пул внутри
//! общего контракта (V4) — без условия; причина — в строке пула (журнал send.pool_cond, карточка сделки).
//!
//! Разбор слов и сверка с состоянием — exec/pool_cond_decode.rs (раскладка и источники — в его шапке).
//! Кто вызывает: exec/pool_cond_read.rs (fresh).
//! От чего зависит: exec/{pool_cond, pool_cond_row, pool_cond_decode}.rs, node/{rpc, call}.rs, quote/route_calc.rs
//! (Leg), ctx.rs.
//! Настройки: нет (срок — verify.timeout_ms передаёт вызывающий).

use std::time::{Duration, Instant};

use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::node::rpc;
use super::super::quote::route_calc::Leg;
use super::pool_cond::How;
use super::pool_cond_decode::{differ, words};
use super::pool_cond_row::{PoolConds, root};

/// Адрес условия правила. Ошибок нет.
fn addr(h: &How) -> Option<Address> {
    match h {
        How::Slots(a, _, _) | How::Root(a) => Some(*a),
        How::Off(_) => None,
    }
}

/// Самопроверка и замена (шапка). Принимает: окружение, (правила по пулам, ссылки пулов по тому же порядку), свежие
/// ноги на блоке плана, условие (меняется на месте), срок чтения корня. Ошибок нет: сбой — строка «без условия».
pub async fn apply(
    ctx: &Ctx,
    (rules, refs): (&[(String, How)], &[PoolRef]),
    legs: &[Leg],
    conds: &mut PoolConds,
    lim: Duration,
) {
    let mut failed: Vec<(usize, String)> = Vec::new();
    for (i, ((_, how), r)) in rules.iter().zip(refs).enumerate() {
        let How::Slots(a, slots, _) = how else {
            continue;
        };
        if conds.rows.get(i).is_none_or(|row| row.cond.is_err()) {
            continue;
        }
        let why = match legs.iter().find(|l| &l.entry.pool.reference == r) {
            None => Some("свежего состояния пула нет".to_string()),
            Some(l) => {
                let e = &l.entry;
                differ(e.family.proto, &e.pool.state, words(*a, slots, &conds.known))
            }
        };
        if let Some(w) = why {
            failed.push((i, format!("самопроверка слотов не сошлась ({w})")));
        }
    }
    if failed.is_empty() {
        return;
    }
    let started = Instant::now();
    let mut addrs: Vec<Address> = failed
        .iter()
        .filter_map(|(i, _)| match &refs[*i] {
            PoolRef::Contract(a) => Some(*a),
            _ => None,
        })
        .collect();
    addrs.sort();
    addrs.dedup();
    let got = roots(ctx, &addrs, conds.block, lim).await;
    for (i, why) in failed {
        let Some(at) = addr(&rules[i].1) else {
            continue;
        };
        let key = format!("{at:#x}");
        match got.iter().find(|(x, _)| *x == at) {
            Some((_, Ok(h))) => {
                // Ревью 439 (4): по корню фильтр разбора проигрыша — любое событие (pool_cond_topics.rs by_cond).
                conds.known.insert(key, Value::String(format!("{h:#x}")));
                let note = format!("корень хранилища (eth_getProof storageHash): {why}");
                mark(rules, conds, at, Ok(note));
            }
            Some((_, Err(e))) => {
                conds.known.remove(&key);
                mark(rules, conds, at, Err(format!("{why}; корень не прочитан: {e}")));
            }
            None => {
                // Пул внутри общего контракта (V4): корень общий — убрать только слоты этого пула.
                if let (How::Slots(_, slots, _), Some(Value::Object(m))) =
                    (&rules[i].1, conds.known.get_mut(&key))
                {
                    for s in slots {
                        m.remove(&format!("{s:#x}"));
                    }
                    if m.is_empty() {
                        conds.known.remove(&key);
                    }
                }
                let why = format!("{why}; пул внутри общего контракта — без условия");
                conds.rows[i].cond = Err(why);
            }
        }
    }
    conds.read_ms += started.elapsed().as_millis();
}

/// Строки всех пулов этого адреса — новое условие (адрес целиком: корень или без условия). Ошибок нет.
fn mark(rules: &[(String, How)], conds: &mut PoolConds, a: Address, cond: Result<String, String>) {
    for (row, (_, h)) in conds.rows.iter_mut().zip(rules) {
        if addr(h) == Some(a) {
            row.cond = cond.clone();
        }
    }
}

/// Корни хранилища адресов одной пачкой eth_getProof на блоке. Возвращает: (адрес, корень или причина). Ошибок нет.
async fn roots(
    ctx: &Ctx,
    addrs: &[Address],
    block: u64,
    lim: Duration,
) -> Vec<(Address, Result<B256, String>)> {
    if addrs.is_empty() {
        return Vec::new();
    }
    let tag = block_tag(block);
    let calls: Vec<(&str, Value)> = addrs
        .iter()
        .map(|a| ("eth_getProof", json!([a, [], tag])))
        .collect();
    let res = match tokio::time::timeout(lim, rpc::batch(ctx, &calls)).await {
        Ok(r) => r.map_err(|e| e.message),
        Err(_) => Err("срок verify.timeout_ms вышел".to_string()),
    };
    let mut got = match res {
        Ok(r) => r.into_iter(),
        Err(why) => {
            let all = addrs.iter().map(|a| (*a, Err(why.clone())));
            return all.collect();
        }
    };
    let mut out = Vec::with_capacity(addrs.len());
    for a in addrs {
        let r = match got.next() {
            Some(r) => r.and_then(|v| root(ctx, &v)).map_err(|e| e.message),
            None => Err("нет ответа на запрос пачки".to_string()),
        };
        out.push((*a, r));
    }
    out
}
