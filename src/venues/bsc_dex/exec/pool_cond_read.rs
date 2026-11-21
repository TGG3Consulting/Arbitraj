//! Файл: venues/bsc_dex/exec/pool_cond_read.rs
//! Что делает: решение 438 — условие пулов маршрута для условной отправки в секвенсор Arbitrum
//! (eth_sendRawTransactionConditional, knownAccounts): проигрыш «конкурент изменил пул раньше нас» становится
//! бесплатным отказом −32003 вместо отката с оплатой газа. Значения — на ТОМ ЖЕ блоке, на котором перечитаны пулы
//! цикла (блок плана, quote/cycle_legs.rs fresh_at): значения с более нового блока пропустили бы состояние, которого
//! план не видел. Чтение — одна пачка JSON-RPC (eth_getStorageAt по слотам, eth_getProof по корням — правила
//! exec/pool_cond.rs) ОДНОВРЕМЕННО с перечтением пулов (Multicall3) на этом блоке: лишнего круга до узла перед
//! отправкой нет; время чтения — в журнал сделки (send.pool_cond).
//! Читается, только если send.pool_cond_mode = pools (решение 439; не задана или off — условия пулов нет, значения
//! пулов не читаются вовсе, лишних запросов к узлу нет; условная отправка — только с окном времени, решение 436) и у
//! площадки есть включённый канал sequencer с окном channel.cond_time_window_s (send/pool_cond_mode.rs wanted, ревью
//! 438 (F) — и только при dex.gas_mode = eip1559_l1): нет окна — нет условной отправки, нет и условий пулов (снять
//! после 17.01.2027 = снять окно). Пул без подтверждённых слотов, сбой или срок чтения — сделка идёт без условия на этот
//! пул, причина в строке. Ревью 438 (C): прочитанные слова сверяются с перечитанным состоянием пула
//! (exec/pool_cond_check.rs) — расхождение: корень хранилища или без условия.
//! Кто вызывает: exec/cycle_trade.rs, exec/cycle_exec.rs (перечтение после разрешения).
//! От чего зависит: exec/{pool_cond, pool_cond_row, pool_cond_check}.rs, quote/{cycle_legs, verify}.rs, node/{rpc,
//! call}.rs, ctx.rs.
//! Настройки: verify.timeout_ms (срок чтения — тот же, что у перечтения пулов) [площадка DEX]; режим и каналы —
//! send/pool_cond_mode.rs wanted (вызывающий).
//!
//! Методы (nitro-geth 260bc5dcbd5a16925d7e26145577b2612f30f428): eth_getStorageAt — internal/ethapi/api.go:603-619
//! (state.GetState — та же функция, что проверка слота, arbitrum_types/txoptions.go:100); eth_getProof —
//! internal/ethapi/api.go:366-420, поле storageHash = statedb.GetStorageRoot(address) (:385, :342) — та же функция, что
//! проверка корня (txoptions.go:93); оба в пространстве eth узла Nitro (arbitrum/apibackend.go:168 ethapi.GetAPIs).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use alloy_primitives::{Address, B256};
use serde_json::{Map, Value, json};

use crate::core::types::{DexCycle, PoolRef};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::node::rpc;
use super::super::pools::registry::Entry;
use super::super::quote::route_calc::Leg;
use super::super::quote::{Quoting, cycle_legs, verify};
use super::pool_cond::{self, How};
use super::pool_cond_check;
use super::pool_cond_row::{PoolConds, Row, failed, root, word};

/// Ноги цикла на свежем состоянии и (если want) условие пулов на том же блоке — одновременно.
/// Принимает: котировку, цикл, нужно ли условие (wanted). Возвращает: ноги, блок, условие (None — не нужно).
/// Ошибки: как cycle_legs::fresh (сбой чтения условия — не ошибка: строки «без условия» с причиной).
pub async fn fresh(
    q: &Quoting,
    c: &DexCycle,
    want: bool,
) -> GatewayResult<(Vec<Leg>, u64, Option<PoolConds>)> {
    if !want {
        let (legs, block) = cycle_legs::fresh(q, c).await?;
        return Ok((legs, block, None));
    }
    let ctx = &q.ctx;
    let entries = cycle_legs::entries(q, c)?;
    let block = cycle_legs::head_block(q)?;
    let (_, _, timeout_ms) = verify::limits(ctx)?;
    let rules: Vec<(String, How)> = entries
        .iter()
        .map(|e| (label(e), pool_cond::rule(e)))
        .collect();
    let refs: Vec<PoolRef> = entries.iter().map(|e| e.pool.reference.clone()).collect();
    let lim = Duration::from_millis(timeout_ms);
    let reading = tokio::time::timeout(lim, read(ctx, &rules, block));
    let (legs, conds) = tokio::join!(cycle_legs::fresh_at(q, c, entries, block), reading);
    let (legs, block) = legs?;
    let mut conds = conds.unwrap_or_else(|_| {
        let why = format!("чтение условия не уложилось в verify.timeout_ms ({timeout_ms} мс)");
        failed(block, &rules, &why, timeout_ms.into())
    });
    // Ревью 438 (C): самопроверка слотов по перечитанному состоянию на том же блоке.
    pool_cond_check::apply(ctx, (&rules, &refs), &legs, &mut conds, lim).await;
    Ok((legs, block, Some(conds)))
}

/// Подпись пула для журнала: DEX, монета, адрес контракта. Ошибок нет.
fn label(e: &Entry) -> String {
    format!(
        "{} {} {:#x}",
        e.family.title,
        e.ticker,
        e.pool.reference.address()
    )
}

/// Чтение значений одной пачкой на блоке плана. Принимает: окружение, правила пулов, блок. Возвращает: условие (сбой
/// пачки — все пулы «без условия» с причиной). Ошибок нет.
async fn read(ctx: &Ctx, rules: &[(String, How)], block: u64) -> PoolConds {
    let started = Instant::now();
    let mut want: BTreeMap<Address, Option<Vec<B256>>> = BTreeMap::new();
    for (_, h) in rules {
        match h {
            How::Slots(a, slots, _) => {
                let list = want.entry(*a).or_insert_with(|| Some(Vec::new()));
                let list = list.get_or_insert_with(Vec::new);
                for s in slots {
                    if !list.contains(s) {
                        list.push(*s);
                    }
                }
            }
            How::Root(a) => {
                want.entry(*a).or_insert(None);
            }
            How::Off(_) => {}
        }
    }
    let tag = block_tag(block);
    let mut calls: Vec<(&str, Value)> = Vec::new();
    for (a, slots) in &want {
        match slots {
            Some(list) => {
                for s in list {
                    calls.push(("eth_getStorageAt", json!([a, s, tag])));
                }
            }
            None => calls.push(("eth_getProof", json!([a, [], tag]))),
        }
    }
    let res = match rpc::batch(ctx, &calls).await {
        Ok(r) => r,
        Err(e) => {
            let why = format!("чтение условия: {}", e.message);
            return failed(block, rules, &why, started.elapsed().as_millis());
        }
    };
    let mut res = res.into_iter();
    let mut known = Map::new();
    let mut bad: BTreeMap<Address, String> = BTreeMap::new();
    for (a, slots) in &want {
        let key = format!("{a:#x}");
        match slots {
            Some(list) => {
                let mut vals = Map::new();
                for s in list {
                    match res.next().map(|r| r.and_then(|v| word(ctx, &v))) {
                        Some(Ok(v)) => {
                            vals.insert(format!("{s:#x}"), Value::String(format!("{v:#x}")));
                        }
                        Some(Err(e)) => {
                            bad.insert(*a, e.message);
                        }
                        None => {
                            bad.insert(*a, "нет ответа на запрос пачки".into());
                        }
                    }
                }
                if !bad.contains_key(a) {
                    known.insert(key, Value::Object(vals));
                }
            }
            None => match res.next().map(|r| r.and_then(|v| root(ctx, &v))) {
                Some(Ok(h)) => {
                    known.insert(key, Value::String(format!("{h:#x}")));
                }
                Some(Err(e)) => {
                    bad.insert(*a, e.message);
                }
                None => {
                    bad.insert(*a, "нет ответа на запрос пачки".into());
                }
            },
        }
    }
    let rows = rules
        .iter()
        .map(|(pool, h)| Row {
            pool: pool.clone(),
            cond: match h {
                How::Off(why) => Err((*why).to_string()),
                How::Slots(a, _, _) | How::Root(a) if bad.contains_key(a) => {
                    Err(format!("значение не прочитано: {}", bad[a]))
                }
                How::Slots(_, _, what) => Ok((*what).to_string()),
                How::Root(a) if matches!(want.get(a), Some(Some(_))) => Ok(format!(
                    "корень не ставится — у адреса одно из двух (txoptions.go:92-104); в условии \
                     только слоты другого шага того же контракта: {}",
                    slots_of(rules, *a)
                )),
                How::Root(_) => Ok("корень хранилища (eth_getProof storageHash)".to_string()),
            },
        })
        .collect();
    PoolConds {
        block,
        known,
        rows,
        read_ms: started.elapsed().as_millis(),
    }
}

/// Ревью 438 (G): что за слоты поставлены у адреса (подписи правил Slots этого адреса). Ошибок нет.
fn slots_of(rules: &[(String, How)], a: Address) -> String {
    let what: Vec<&str> = rules
        .iter()
        .filter_map(|(_, h)| match h {
            How::Slots(x, _, w) if *x == a => Some(*w),
            _ => None,
        })
        .collect();
    what.join("; ")
}
