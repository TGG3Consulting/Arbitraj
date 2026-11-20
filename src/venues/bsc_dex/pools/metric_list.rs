//! Файл: venues/bsc_dex/pools/metric_list.rs
//! Что делает: решение 482 — пулы Metric OMM перечислением фабрики как источник справочника при поиске по фабрикам
//! (index.factory_scan = да), без индексатора: реестра «пул по паре» у фабрики Metric нет, есть номера пулов —
//! nextPoolIdx() и idxToPool(номер), номера 1..nextPoolIdx − 1 (исходник фабрики, abi_metric.rs). Монеты пула —
//! getImmutables() (token0, token1); кандидат — каждая пара (монета подписки, встречный), обе монеты которой в пуле
//! (stable_list::candidates); дальше — index::take → проверка verify_metric.rs (isPool, фабрика, токены) → реестр.
//! Три пачки на проход (номер, адреса, монеты), каждая — Multicall3 частями по dex.load_batch на теге latest; уже
//! прочитанные номера не спрашиваются повторно (память SEEN). Ревью t251: только у DEX без справочника индексатора.
//! Кто вызывает: pools/factory_scan.rs (pass); count / pool_of / tokens_of — ещё tests/arbitrum_forks_482_reg_live.rs.
//! От чего зависит: ctx.rs, families.rs (all, classify, Proto), indexer_gap.rs, abi_metric.rs, node/call.rs (try_chunks),
//! stable_list.rs (Extra, candidates).
//! Настройки: dex.factory [DEX] (через classify — dex.kind, dex.pool_enabled); dex.load_batch [bsc_dex] — размер пачки
//! передаёт factory_scan.rs.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_metric::{IMetricOmmFactory as F, IMetricOmmPool as P};
use super::families::{self, Family, Proto};
use super::indexer_gap;
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

/// Перечисленное у фабрики DEX за время работы процесса: следующий непрочитанный номер и пулы (пул, token0, token1).
/// Ревью t251: проход поиска по фабрикам повторяется при смене настроек dex.* (factory_scan.rs) — номера, уже
/// прочитанные полностью, второй раз не спрашиваются; каждый проход — nextPoolIdx() и только новые номера.
#[derive(Debug, Clone, Default)]
struct Seen {
    next: u64,
    pools: Vec<(Address, Address, Address)>,
}

/// Память перечисления по (DEX, фабрика). Сменили фабрику — новый ключ, перечисление заново.
static SEEN: LazyLock<Mutex<HashMap<(String, Address), Seen>>> = LazyLock::new(Default::default);

/// Копия памяти по ключу (нет — с номера 1). Ошибок нет.
fn seen(id: &str, fa: Address) -> Seen {
    let m = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    m.get(&(id.to_string(), fa))
        .cloned()
        .unwrap_or(Seen { next: 1, pools: Vec::new() })
}

/// Подключённые DEX вида metric_omm без справочника индексатора (indexer_gap.rs — у DEX из его списка, например
/// metric-bsc, пулы даёт индексатор, как раньше) и их фабрики. Возвращает: (DEX, фабрика) и заметки. Ошибок нет.
fn listed(ctx: &Ctx) -> (Vec<(&'static Family, Address)>, Vec<String>) {
    let (mut out, mut notes) = (Vec::new(), Vec::new());
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.proto == Proto::MetricOmm)
    {
        match indexer_gap::outside(ctx, f.id) {
            Some(true) => {}
            Some(false) => continue,
            None => {
                notes.push(indexer_gap::why_not(f.id, None));
                continue;
            }
        }
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

/// Проход источника: nextPoolIdx() всех фабрик одной пачкой, затем новые номера idxToPool одной пачкой, затем
/// getImmutables новых пулов одной пачкой (каждая — try_chunks частями по dex.load_batch). Номера, где узел не ответил,
/// не запоминаются — следующий проход спросит их снова. Принимает: окружение, монеты подписки, встречные, размер
/// пачки. Возвращает: Extra. Ошибок нет (узел не ответил — no_answer, повтор решает factory_scan.rs).
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
    // По DEX: память, число номеров у фабрики (None — не прочитано) и пулы нового прохода.
    let mut state: Vec<(Seen, Option<u64>, Vec<Address>)> = Vec::new();
    let (mut idx, mut owner): (Vec<(Address, Bytes)>, Vec<usize>) = (Vec::new(), Vec::new());
    let got = call::try_chunks(ctx, &first, "latest", size).await;
    for (k, ((f, fa), a)) in dexes.iter().zip(got).enumerate() {
        let old = seen(f.id, *fa);
        let n = match bytes(a, "nextPoolIdx()").and_then(|b| count(&b).map_err(Some)) {
            Ok(n) => Some(n),
            Err(Some(why)) => {
                x.notes.push(format!("{}: {why}", f.id));
                None
            }
            Err(None) => {
                x.no_answer += 1;
                None
            }
        };
        for i in old.next..n.unwrap_or(0) {
            idx.push((*fa, F::idxToPoolCall { idx: U256::from(i) }.abi_encode().into()));
            owner.push(k);
        }
        state.push((old, n, Vec::new()));
    }
    let mut full = vec![true; dexes.len()];
    let got = call::try_chunks(ctx, &idx, "latest", size).await;
    for (&k, a) in owner.iter().zip(got) {
        match bytes(a, "idxToPool").and_then(|b| pool_of(&b).map_err(Some)) {
            Ok(p) if p != Address::ZERO => state[k].2.push(p),
            Ok(_) => {}
            Err(Some(why)) => x.notes.push(format!("{}: {why}", dexes[k].0.id)),
            Err(None) => {
                x.no_answer += 1;
                full[k] = false;
            }
        }
    }
    let new: Vec<(usize, Address)> = state
        .iter()
        .enumerate()
        .flat_map(|(k, s)| s.2.iter().map(move |p| (k, *p)))
        .collect();
    let imm: Vec<(Address, Bytes)> = new
        .iter()
        .map(|(_, p)| (*p, P::getImmutablesCall {}.abi_encode().into()))
        .collect();
    let got = call::try_chunks(ctx, &imm, "latest", size).await;
    let mut fresh: Vec<Vec<(Address, Address, Address)>> = vec![Vec::new(); dexes.len()];
    for ((k, p), a) in new.iter().zip(got) {
        match bytes(a, "getImmutables()").and_then(|b| tokens_of(&b).map_err(Some)) {
            Ok((_, t0, t1)) => fresh[*k].push((*p, t0, t1)),
            Err(Some(why)) => x.notes.push(format!("{}: пул {p:#x}: {why}", dexes[*k].0.id)),
            Err(None) => {
                x.no_answer += 1;
                full[*k] = false;
            }
        }
    }
    for (k, (f, fa)) in dexes.iter().enumerate() {
        let (old, n, _) = &state[k];
        let mut all = old.pools.clone();
        all.extend(fresh[k].iter().copied());
        let mut pairs = 0;
        for (p, t0, t1) in &all {
            for c in candidates(f.id, *p, &[*t0, *t1], coins, quotes) {
                pairs += 1;
                x.found.entry(c.base).or_default().push(c);
            }
        }
        if let (Some(n), true) = (n, full[k]) {
            let mut m = SEEN.lock().unwrap_or_else(|p| p.into_inner());
            let pools = all.clone();
            m.insert((f.id.to_string(), *fa), Seen { next: (*n).max(old.next), pools });
        }
        let note = format!("{}: пулов фабрики {}, пар построено {pairs}", f.id, all.len());
        x.notes.push(note);
    }
    x.calls = first.len() + idx.len() + imm.len();
    x
}
