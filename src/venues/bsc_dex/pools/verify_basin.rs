//! Файл: venues/bsc_dex/pools/verify_basin.rs
//! Что делает: решение 485 — проверка кандидатов скважин Basin пачкой (решения — verify_basin_judge.rs): слоты ERC-1967
//! всех скважин — одной пачкой eth_getStorageAt; первый проход — те же вызовы, что у чтения бота (load_basin.rs calls:
//! резервы, totalSupply, функция, токены, wellImplementation скважины и реализации из слота, таблицы A); второй —
//! calcReserve функции на сдвинутых резервах = наш расчёт. Пара записи — по возрастанию адреса (как у всех видов);
//! порядок самой скважины нужен обмену — его читает load_basin.rs.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs, node/rpc.rs, basin_conf.rs, load_basin.rs, verify_basin_judge.rs,
//! discover.rs, verify.rs.
//! Настройки: через basin_conf.rs — dex.factory, dex.basin_* [DEX].

use std::str::FromStr;

use alloy_primitives::{Address, B256, Bytes};
use serde_json::{Value, json};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::{call, rpc};
use super::basin_conf::{self, BasinConf};
use super::discover::{Found, sorted};
use super::load_basin;
use super::verify::{Candidate, Verdict};
use super::verify_basin_judge::{self as judge, First};

/// Ответы части вызовов (сбой узла — Err всей части). Ошибок нет.
fn take(
    got: &mut impl Iterator<Item = call::Piece>,
    n: usize,
) -> Result<Vec<Result<Bytes, String>>, String> {
    got.by_ref().take(n).collect()
}

/// Слоты ERC-1967 скважин одной пачкой. Возвращает: реализацию по каждой (Err — узел или ответ не слово).
async fn slots(ctx: &Ctx, wells: &[Address]) -> Vec<Result<Address, String>> {
    let reqs: Vec<(&str, Value)> = wells
        .iter()
        .map(|w| ("eth_getStorageAt", json!([w, basin_conf::impl_slot(), "latest"])))
        .collect();
    match rpc::batch(ctx, &reqs).await {
        Err(e) => wells.iter().map(|_| Err(e.message.clone())).collect(),
        Ok(list) => list
            .into_iter()
            .map(|r| {
                let raw = r.map_err(|e| e.message)?;
                let w: B256 = serde_json::from_str(raw.get())
                    .map_err(|e| format!("eth_getStorageAt: не слово: {e}"))?;
                Ok(Address::from_word(w))
            })
            .collect(),
    }
}

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (настройки не заданы, не адрес — отказ с
/// причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut out: Vec<Option<Verdict>> = vec![None; cands.len()];
    let mut plans: Vec<(usize, Address, BasinConf)> = Vec::new();
    for (k, c) in cands.iter().enumerate() {
        let plan = basin_conf::read(ctx, c.family.id).and_then(|conf| {
            Address::from_str(&c.pool)
                .map(|w| (w, conf))
                .map_err(|_| format!("«{}» — не адрес скважины Basin", c.pool))
        });
        match plan {
            Ok((w, conf)) => plans.push((k, w, conf)),
            Err(why) => out[k] = Some(Verdict::Rejected(why)),
        }
    }
    let wells: Vec<Address> = plans.iter().map(|(_, w, _)| *w).collect();
    let slot = slots(ctx, &wells).await;
    // Первый проход: вызовы чтения бота.
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    let mut ready = Vec::new();
    for ((k, w, conf), s) in plans.into_iter().zip(slot) {
        match s {
            Err(node) => out[k] = Some(Verdict::Failed(node)),
            Ok(si) => {
                let c = load_basin::calls(w, &conf, si);
                ready.push((k, w, conf, si, c.len()));
                calls.extend(c);
            }
        }
    }
    let mut got = call::try_chunks(ctx, &calls, "latest", size).await.into_iter();
    let mut firsts: Vec<(usize, Address, First)> = Vec::new();
    for (k, w, conf, si, n) in ready {
        let c = &cands[k];
        let pair = (c.coin, c.quote);
        let r = take(&mut got, n).map(|a| {
            load_basin::decode(&a, si).and_then(|raw| judge::first(w, pair, &conf, &raw))
        });
        match r {
            Err(node) => out[k] = Some(Verdict::Failed(node)),
            Ok(Ok(f)) => firsts.push((k, w, f)),
            Ok(Err(why)) => out[k] = Some(Verdict::Rejected(why)),
        }
    }
    // Второй проход: calcReserve на сдвинутых резервах.
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    let mut seconds = Vec::new();
    for (k, w, f) in firsts {
        match judge::second_calls(&f) {
            Ok(sc) => {
                seconds.push((k, w, f, sc.len()));
                calls.extend(sc);
            }
            Err(why) => out[k] = Some(Verdict::Rejected(format!("скважина {w:#x}: {why}"))),
        }
    }
    let mut got = call::try_chunks(ctx, &calls, "latest", size).await.into_iter();
    for (k, w, f, n) in seconds {
        out[k] = Some(match take(&mut got, n) {
            Err(node) => Verdict::Failed(node),
            Ok(a) => match judge::second(&f, &a) {
                Ok(()) => Verdict::Taken(Box::new(found(&cands[k], w))),
                Err(why) => Verdict::Rejected(format!("скважина {w:#x}: {why}")),
            },
        });
    }
    let lost = || Verdict::Failed("проверка скважины не дошла до итога".into());
    out.into_iter().map(|v| v.unwrap_or_else(lost)).collect()
}

/// Найденный пул: ссылка — адрес скважины, пара — по возрастанию адреса. Ошибок нет.
fn found(c: &Candidate, well: Address) -> Found {
    let (token0, token1) = sorted(c.coin, c.quote);
    Found {
        family: c.family,
        reference: PoolRef::Contract(well),
        token0,
        token1,
        coin: c.coin,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    }
}
