//! Файл: venues/bsc_dex/pools/verify.rs
//! Что делает: проверка пула из справочника индексатора (или из события создания) на своём узле перед тем, как его
//! слушать (решение 58; CLAUDE.md: «адрес контракта у биржи совпадает с адресом токена в пуле»). V2/V3: два прохода
//! Multicall3 — неизменяемые поля пула (factory, token0, token1; V3 — fee, tickSpacing и lmPool() — ABI PancakeSwap
//! или Uniswap; V2 со своей комиссией — метод dex.v2_fee_method), затем реестр фабрики DEX (getPair / getPool) —
//! именно он подтверждает, что это пул этой фабрики (поддельный контракт может вернуть из factory() что угодно).
//! V4/Infinity — verify_v4.rs. Вызовы — частями по dex.verify_batch: сбой узла на части — итог «не проверен» только
//! у её пулов (повтор — pools/index.rs), откат вызова — это ответ (пул отклонён с причиной).
//! Кто вызывает: pools/index.rs, pools/created_log.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_check.rs, abi_v2v3.rs, discover.rs (Found), verify_rules.rs,
//! verify_calls.rs, verify_v4.rs.
//! Настройки: dex.verify_batch, dex.multicall3 [bsc_dex]; dex.factory, dex.v2_fee_source, dex.v2_fee_method,
//! dex.v2_fee_denominator [DEX].
//!
//! ABI — abi_check.rs (ссылки там); getPair / getPool — abi_v2v3.rs (IUniswapV2Factory.sol#L9, IUniswapV3Factory.sol#L48).

use std::str::FromStr;

use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call::{self, Piece};
use super::discover::Found;
use super::families::{Family, Proto};
pub use super::verify_calls::{fee_method, field_calls, registry_call};
pub use super::verify_rules::{Read, check, fill, registered, v2_fee_pips, v3_abi};
use super::{verify_algebra, verify_dodo, verify_v4};

/// Кандидат на проверку: DEX (вид), пул как у источника (адрес или PoolId), монета и встречный токен.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub family: &'static Family,
    pub pool: String,
    pub coin: Address,
    pub quote: Address,
}

/// Итог проверки пула.
#[derive(Debug, Clone)]
pub enum Verdict {
    /// Прошёл — слушать.
    Taken(Box<Found>),
    /// Не прошёл — причина (на экран и один раз в журнал).
    Rejected(String),
    /// Узел не ответил (связь, 429) — «не проверен», повтор.
    Failed(String),
}

/// Размер части проверки: dex.verify_batch. Ошибки: NotConfigured (не задана или 0).
pub fn batch(ctx: &Ctx) -> GatewayResult<usize> {
    match ctx.int("dex.verify_batch")? {
        0 => Err(ctx.not_set(
            "dex.verify_batch",
            "dex.verify_batch = 0 — задайте число больше нуля",
        )),
        n => Ok(n as usize),
    }
}

/// Проверка пачки кандидатов. Возвращает: итог по каждому в том же порядке. Ошибки: NotConfigured (dex.verify_batch).
pub async fn verify(ctx: &Ctx, cands: &[Candidate]) -> GatewayResult<Vec<Verdict>> {
    let size = batch(ctx)?;
    let mut out: Vec<Option<Verdict>> = vec![None; cands.len()];
    // Пути проверки: V4/Infinity (ключ у менеджера), DODO PMM (реестр фабрики), Algebra (фабрика poolByPair),
    // остальные V2/V3 и KyberSwap Elastic (поля пула + реестр фабрики).
    let mut v4 = Vec::new();
    let mut dodo = Vec::new();
    let mut algebra = Vec::new();
    let mut v2v3 = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        match c.family.proto {
            Proto::InfinityCl | Proto::InfinityBin | Proto::UniswapV4 => v4.push(i),
            p if p.is_dodo() => dodo.push(i),
            Proto::Algebra => algebra.push(i),
            _ => v2v3.push(i),
        }
    }
    let list: Vec<Candidate> = v4.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in v4
        .into_iter()
        .zip(verify_v4::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    let list: Vec<Candidate> = dodo.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in dodo
        .into_iter()
        .zip(verify_dodo::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    let list: Vec<Candidate> = algebra.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in algebra
        .into_iter()
        .zip(verify_algebra::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    let list: Vec<Candidate> = v2v3.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in v2v3.into_iter().zip(contracts(ctx, &list, size).await) {
        out[i] = Some(r);
    }
    Ok(out.into_iter().flatten().collect())
}

/// Итог по ответам части вызовов: сбой узла хоть на одном — Failed. Иначе — ответы как есть. Ошибок нет.
pub fn answers(got: &[Piece]) -> Result<Vec<Result<Bytes, String>>, String> {
    got.iter().cloned().collect()
}

/// Пулы V2/V3: поля пула, затем реестр фабрики. Ошибок нет: итог — у каждого пула.
async fn contracts(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<(Address, usize, usize), String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = Address::from_str(&c.pool)
            .map_err(|_| format!("«{}» — не адрес контракта пула", c.pool))
            .and_then(|a| field_calls(ctx, c.family).map(|l| (a, l)));
        plans.push(plan.map(|(a, list)| {
            let at = calls.len();
            calls.extend(list.iter().map(|d| (a, d.clone())));
            (a, at, list.len())
        }));
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    // Первый проход: поля пула и сверка с настройками.
    let mut first: Vec<Result<(Address, Read), Verdict>> = Vec::new();
    let mut reg = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let r = match plan {
            Err(w) => Err(Verdict::Rejected(w)),
            Ok((a, at, n)) => match answers(&got[at..at + n]) {
                Err(node) => Err(Verdict::Failed(node)),
                Ok(list) => fields(ctx, c, &list)
                    .map(|read| (a, read))
                    .map_err(Verdict::Rejected),
            },
        };
        if let Ok((_, read)) = &r {
            let factory = ctx.dex_addr("dex.factory", c.family.id).unwrap_or_default();
            reg.push((factory, registry_call(c.family, read)));
        }
        first.push(r);
    }
    // Второй проход: реестр фабрики должен вернуть тот же адрес.
    let mut regs = call::try_chunks(ctx, &reg, "latest", size)
        .await
        .into_iter();
    let mut out = Vec::new();
    for (c, r) in cands.iter().zip(first) {
        out.push(match r {
            Err(v) => v,
            Ok((a, read)) => match regs.next() {
                None => Verdict::Failed("нет ответа реестра".into()),
                Some(Err(node)) => Verdict::Failed(node),
                Some(Ok(Err(rev))) => Verdict::Rejected(format!("реестр фабрики откатился: {rev}")),
                Some(Ok(Ok(b))) => match registered(a, &b).and_then(|_| found(ctx, c, a, &read)) {
                    Ok(f) => Verdict::Taken(Box::new(f)),
                    Err(w) => Verdict::Rejected(w),
                },
            },
        });
    }
    out
}

/// Поля пула по ответам и сверка с фабрикой DEX и монетой. Ошибка — причина отказа.
fn fields(ctx: &Ctx, c: &Candidate, list: &[Result<Bytes, String>]) -> Result<Read, String> {
    let mut read = Read::default();
    fill(c.family, &mut read, list)?;
    let expected = ctx
        .dex_addr("dex.factory", c.family.id)
        .map_err(|e| e.message)?;
    check(c, expected, &read)?;
    Ok(read)
}

/// Найденный пул V2/V3 по прочитанному. Ошибка — не задан делитель комиссии или комиссия неверна.
fn found(ctx: &Ctx, c: &Candidate, pool: Address, r: &Read) -> Result<Found, String> {
    let fee_pips = match (c.family.proto, r.v2_fee) {
        (Proto::V2, Some(v)) => {
            let den = ctx
                .dex_int("dex.v2_fee_denominator", c.family.id)
                .map_err(|e| e.message)?;
            Some(v2_fee_pips(v, den)?)
        }
        (Proto::V2, None) => None,
        _ => r.fee,
    };
    Ok(Found {
        family: c.family,
        reference: PoolRef::Contract(pool),
        token0: r.token0,
        token1: r.token1,
        coin: c.coin,
        fee_pips,
        tick_spacing: r.spacing,
        bin_step: None,
        key: None,
        hooks: None,
    })
}
