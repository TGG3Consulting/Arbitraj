//! Файл: venues/bsc_dex/pools/verify_basin.rs
//! Что делает: решение 485 — проверка кандидатов скважин Basin пачкой в два прохода (решения — verify_basin_judge.rs):
//! первый — реестр Aquifer, прокси, токены, функция, резервы; второй — реализация прокси в Aquifer и сверка
//! calcLpTokenSupply функции скважины с нашим расчётом. Пара записи — по возрастанию адреса (как у всех видов); порядок
//! самой скважины нужен обмену — его читает load_basin.rs.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs, basin_conf.rs, verify_basin_judge.rs, discover.rs, verify.rs.
//! Настройки: через basin_conf.rs — dex.factory, dex.basin_* [DEX].

use std::str::FromStr;

use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::basin_conf;
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};
use super::verify_basin_judge::{self as judge, First};

/// Ответы части вызовов (сбой узла — Err всей части). Ошибок нет.
fn take(
    got: &mut impl Iterator<Item = call::Piece>,
    n: usize,
) -> Result<Vec<Result<Bytes, String>>, String> {
    got.by_ref().take(n).collect()
}

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (настройки не заданы, не адрес — отказ с
/// причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut out: Vec<Option<Verdict>> = vec![None; cands.len()];
    // Первый проход.
    let mut plans = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for (k, c) in cands.iter().enumerate() {
        let plan = basin_conf::read(ctx, c.family.id).and_then(|conf| {
            Address::from_str(&c.pool)
                .map(|w| (w, conf))
                .map_err(|_| format!("«{}» — не адрес скважины Basin", c.pool))
        });
        match plan {
            Ok((w, conf)) => {
                let fc = judge::first_calls(w, conf.aquifer, conf.lut);
                plans.push((k, w, conf, fc.len()));
                calls.extend(fc);
            }
            Err(why) => out[k] = Some(Verdict::Rejected(why)),
        }
    }
    let mut got = call::try_chunks(ctx, &calls, "latest", size).await.into_iter();
    let mut firsts: Vec<(usize, Address, basin_conf::BasinConf, First)> = Vec::new();
    for (k, w, conf, n) in plans {
        let c = &cands[k];
        match take(&mut got, n) {
            Err(node) => out[k] = Some(Verdict::Failed(node)),
            Ok(a) => match judge::first(w, (c.coin, c.quote), &conf, &a) {
                Ok(f) => firsts.push((k, w, conf, f)),
                Err(why) => out[k] = Some(Verdict::Rejected(why)),
            },
        }
    }
    // Второй проход.
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    let mut lens = Vec::new();
    for (_, _, conf, f) in &firsts {
        let sc = judge::second_calls(f, conf);
        lens.push(sc.len());
        calls.extend(sc);
    }
    let mut got = call::try_chunks(ctx, &calls, "latest", size).await.into_iter();
    for ((k, w, conf, f), n) in firsts.into_iter().zip(lens) {
        let c = &cands[k];
        out[k] = Some(match take(&mut got, n) {
            Err(node) => Verdict::Failed(node),
            Ok(a) => match judge::second(&f, &conf, &a) {
                Ok(()) => Verdict::Taken(Box::new(found(c, w))),
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
