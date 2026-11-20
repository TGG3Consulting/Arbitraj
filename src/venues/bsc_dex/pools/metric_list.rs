//! Файл: venues/bsc_dex/pools/metric_list.rs
//! Что делает: решение 482 — пулы Metric OMM перечислением фабрики как источник справочника при поиске по фабрикам
//! (index.factory_scan = да), без индексатора: реестра «пул по паре» у фабрики Metric нет, есть номера пулов —
//! nextPoolIdx() и idxToPool(номер), номера 1..nextPoolIdx − 1 (исходник фабрики, abi_metric.rs). Монеты пула —
//! getImmutables() (token0, token1); кандидат — каждая пара (монета подписки, встречный), обе монеты которой в пуле
//! (stable_list::candidates); дальше — index::take → проверка verify_metric.rs (isPool, фабрика, токены) → реестр.
//! Три пачки на проход (номер, адреса, монеты), каждая — Multicall3 частями по dex.load_batch на теге latest.
//! Кто вызывает: pools/factory_scan.rs (pass); count / pool_of / tokens_of — ещё tests/arbitrum_forks_482_live.rs.
//! От чего зависит: ctx.rs, families.rs (all, classify, Proto), abi_metric.rs, node/call.rs (try_chunks),
//! stable_list.rs (Extra, candidates).
//! Настройки: dex.factory [DEX] (через classify — dex.kind, dex.pool_enabled); dex.load_batch [bsc_dex] — размер пачки
//! передаёт factory_scan.rs.

use std::collections::HashMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_metric::{IMetricOmmFactory as F, IMetricOmmPool as P};
use super::families::{self, Family, Proto};
use super::stable_list::{Extra, candidates};

/// Число пулов фабрики по ответу nextPoolIdx(): номера 1..n − 1 (n = 0 или 1 — пулов нет). Принимает: байты ответа.
/// Возвращает: n или текст «не по ABI / слишком велико». Ошибок нет.
pub fn count(raw: &[u8]) -> Result<u64, String> {
    let n = F::nextPoolIdxCall::abi_decode_returns(raw)
        .map_err(|e| format!("ответ nextPoolIdx() не по ABI: {e}"))?;
    if n > U256::from(u64::MAX) {
        return Err(format!("nextPoolIdx() = {n} — больше u64"));
    }
    Ok(n.to::<u64>())
}

/// Адрес пула по ответу idxToPool(номер). Возвращает: адрес (нулевой — номера нет) или текст «не по ABI».
pub fn pool_of(raw: &[u8]) -> Result<Address, String> {
    F::idxToPoolCall::abi_decode_returns(raw).map_err(|e| format!("ответ idxToPool не по ABI: {e}"))
}

/// Монеты пула по ответу getImmutables(): (фабрика, token0, token1). Возвращает: тройку или текст «не по ABI».
pub fn tokens_of(raw: &[u8]) -> Result<(Address, Address, Address), String> {
    let im = P::getImmutablesCall::abi_decode_returns(raw)
        .map_err(|e| format!("ответ getImmutables() не по ABI: {e}"))?;
    Ok((im.factory, im.token0, im.token1))
}

/// Подключённые DEX вида metric_omm и их фабрики. Возвращает: (DEX, фабрика) и заметки. Ошибок нет.
fn listed(ctx: &Ctx) -> (Vec<(&'static Family, Address)>, Vec<String>) {
    let (mut out, mut notes) = (Vec::new(), Vec::new());
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.proto == Proto::MetricOmm)
    {
        let fa = families::classify(ctx, f.id)
            .and_then(|_| ctx.dex_addr("dex.factory", f.id).map_err(|e| e.message));
        match fa {
            Ok(fa) => out.push((f, fa)),
            Err(why) => notes.push(format!("{}: пулы фабрики не перечисляются — {why}", f.id)),
        }
    }
    (out, notes)
}

/// Ответ пачки → байты или причина. Принимает: ответ try_chunks, что спрашивали. Возвращает: Ok(байты),
/// Err(Some(откат)) или Err(None) — узел не ответил. Ошибок нет.
fn bytes(piece: call::Piece, what: &str) -> Result<Bytes, Option<String>> {
    match piece {
        Ok(Ok(b)) => Ok(b),
        Ok(Err(r)) => Err(Some(format!("{what}: откат — {r}"))),
        Err(_) => Err(None),
    }
}

/// Проход источника. Принимает: окружение, монеты подписки, встречные, размер пачки (dex.load_batch).
/// Возвращает: Extra. Ошибок нет (узел не ответил — no_answer, повтор решает factory_scan.rs).
pub async fn scan(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    size: usize,
) -> Extra {
    let (dexes, notes) = listed(ctx);
    let mut x = Extra {
        notes,
        ..Extra::default()
    };
    if dexes.is_empty() {
        return x;
    }
    let first: Vec<(Address, Bytes)> = dexes
        .iter()
        .map(|(_, fa)| (*fa, F::nextPoolIdxCall {}.abi_encode().into()))
        .collect();
    let mut idx: Vec<(Address, Bytes)> = Vec::new();
    let mut owner: Vec<&'static Family> = Vec::new();
    let got = call::try_chunks(ctx, &first, "latest", size).await;
    for ((f, fa), a) in dexes.iter().zip(got) {
        match bytes(a, "nextPoolIdx()").and_then(|b| count(&b).map_err(Some)) {
            Ok(n) => {
                for i in 1..n {
                    let data = F::idxToPoolCall { idx: U256::from(i) }.abi_encode();
                    idx.push((*fa, data.into()));
                    owner.push(*f);
                }
            }
            Err(Some(why)) => x.notes.push(format!("{}: {why}", f.id)),
            Err(None) => x.no_answer += 1,
        }
    }
    let mut pools: Vec<(&'static Family, Address)> = Vec::new();
    let got = call::try_chunks(ctx, &idx, "latest", size).await;
    for (f, a) in owner.iter().zip(got) {
        match bytes(a, "idxToPool").and_then(|b| pool_of(&b).map_err(Some)) {
            Ok(p) if p != Address::ZERO => pools.push((*f, p)),
            Ok(_) => {}
            Err(Some(why)) => x.notes.push(format!("{}: {why}", f.id)),
            Err(None) => x.no_answer += 1,
        }
    }
    let imm: Vec<(Address, Bytes)> = pools
        .iter()
        .map(|(_, p)| (*p, P::getImmutablesCall {}.abi_encode().into()))
        .collect();
    let mut built: HashMap<&'static str, usize> = HashMap::new();
    let got = call::try_chunks(ctx, &imm, "latest", size).await;
    for ((f, p), a) in pools.iter().zip(got) {
        match bytes(a, "getImmutables()").and_then(|b| tokens_of(&b).map_err(Some)) {
            Ok((_, t0, t1)) => {
                let list = candidates(f.id, *p, &[t0, t1], coins, quotes);
                *built.entry(f.id).or_default() += list.len();
                for c in list {
                    x.found.entry(c.base).or_default().push(c);
                }
            }
            Err(Some(why)) => x.notes.push(format!("{}: пул {p:#x}: {why}", f.id)),
            Err(None) => x.no_answer += 1,
        }
    }
    for (f, _) in &dexes {
        let n = pools.iter().filter(|(g, _)| g.id == f.id).count();
        let pairs = built.get(f.id).copied().unwrap_or(0);
        let note = format!("{}: пулов фабрики (idxToPool) {n}, пар построено {pairs}", f.id);
        x.notes.push(note);
    }
    x.calls = first.len() + idx.len() + imm.len();
    x
}
