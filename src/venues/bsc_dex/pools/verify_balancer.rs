//! Файл: venues/bsc_dex/pools/verify_balancer.rs
//! Что делает: проверка пула Balancer V2 (решение 289) из справочника на своём узле. Кто подтверждает пул (решение 59):
//! (1) пул называет своё хранилище getVault() = dex.vault и свой id getPoolId(); (2) хранилище по этому id отдаёт
//! getPool(id) = (этот же адрес, специализация): getPool откатывает незарегистрированный id, а адрес берёт из старших
//! 20 байт id, и id со своим адресом регистрирует только сам пул (registerPool — msg.sender) — самозванец, назвавший
//! чужой id, получает чужой адрес, свой выдуманный id — откат; (3) пул создан фабрикой DEX — isPoolFromFactory у dex.factory (весовой пул) или у
//! dex.balancer_stable_factory (стейблсвап): фабрика создаёт пулы одним кодом, по исходнику которого сделана математика
//! (пулы ACSI с кодом 19 408 байт не из этих фабрик — не берутся); (4) монета и встречный токен — среди токенов пула
//! getPoolTokens(id) (CLAUDE.md: адрес токена у биржи = адрес в пуле). Два прохода частями по dex.verify_batch.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_balancer.rs, discover.rs, verify.rs.
//! Настройки: dex.vault, dex.factory, dex.balancer_stable_factory (не задана — стейблсвап-пулы не берутся) [DEX].
//!
//! Источник: IVault.sol L240 (getPool), L334-L341 (getPoolTokens); PoolRegistry.sol L67-L87 (registerPool: id = адрес
//! msg.sender | специализация | номер), L89-L97 (getPool — withRegisteredPool), L115-L138 (раскладка id); BasePool.sol L155-L161 (getVault, getPoolId);
//! BasePoolFactory.sol L48-L60 (isPoolFromFactory — только пулы, зарегистрированные этой фабрикой при создании).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_balancer::{IBalancerFactory as F, IBalancerPool as P, IBalancerVault as V};
use super::discover::{Found, sorted};
use super::verify::{Candidate, Verdict};

/// Адреса проверки: пул, хранилище, фабрика весовых, фабрика стейблсвапов (может быть не задана).
type Plan = (Address, Address, Address, Option<Address>);

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (не задана настройка, пул не прошёл — отказ с
/// причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let plans: Vec<Result<Plan, String>> = cands.iter().map(|c| plan(ctx, c)).collect();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for p in plans.iter().flatten() {
        calls.push((p.0, P::getVaultCall {}.abi_encode().into()));
        calls.push((p.0, P::getPoolIdCall {}.abi_encode().into()));
    }
    let first = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut it = first.chunks(2);
    let mut ids: Vec<Result<B256, Verdict>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for p in &plans {
        let Ok(p) = p else {
            ids.push(Err(Verdict::Rejected(String::new())));
            continue;
        };
        let part = it.next().unwrap_or(&[]);
        let r = match (part.first(), part.get(1)) {
            (Some(Err(node)), _) | (_, Some(Err(node))) => Err(Verdict::Failed(node.clone())),
            (Some(Ok(v)), Some(Ok(i))) => first_pass(p, v, i).map_err(Verdict::Rejected),
            _ => Err(Verdict::Failed("нет ответа узла".into())),
        };
        if let Ok(id) = &r {
            calls.extend(second_calls(p, *id));
        }
        ids.push(r);
    }
    let second = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut second = second.into_iter();
    let mut out = Vec::new();
    for ((c, p), id) in cands.iter().zip(plans).zip(ids) {
        out.push(match (p, id) {
            (Err(w), _) => Verdict::Rejected(w),
            (_, Err(v)) => v,
            (Ok(p), Ok(id)) => {
                let part: Vec<call::Piece> =
                    second.by_ref().take(second_calls(&p, id).len()).collect();
                match part.into_iter().collect::<Result<Vec<_>, String>>() {
                    Err(node) => Verdict::Failed(node),
                    Ok(a) => match decide(c, p, &a) {
                        Ok(f) => Verdict::Taken(Box::new(f)),
                        Err(w) => Verdict::Rejected(w),
                    },
                }
            }
        });
    }
    out
}

/// Адреса проверки кандидата. Возвращает: план или причину отказа (не адрес, не задана dex.vault / dex.factory).
fn plan(ctx: &Ctx, c: &Candidate) -> Result<Plan, String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула Balancer", c.pool))?;
    let vault = ctx
        .dex_addr("dex.vault", c.family.id)
        .map_err(|e| e.message)?;
    let wf = ctx
        .dex_addr("dex.factory", c.family.id)
        .map_err(|e| e.message)?;
    let sf = ctx
        .dex_addr("dex.balancer_stable_factory", c.family.id)
        .ok();
    Ok((pool, vault, wf, sf))
}

/// Первый проход (без сети): getVault() пула = хранилище из настройки, id из getPoolId(). Принимает: план, ответы
/// getVault и getPoolId (откат — Err с текстом). Возвращает: id или причину отказа.
pub fn first_pass(
    p: &Plan,
    vault: &Result<Bytes, String>,
    id: &Result<Bytes, String>,
) -> Result<B256, String> {
    let (pool, want, _, _) = *p;
    let v = vault
        .as_ref()
        .map_err(|r| format!("пул {pool:#x} откатил getVault(): {r}"))?;
    let v =
        P::getVaultCall::abi_decode_returns(v).map_err(|e| format!("getVault() не по ABI: {e}"))?;
    if v != want {
        return Err(format!(
            "пул {pool:#x} называет хранилище {v:#x}, а dex.vault — {want:#x}"
        ));
    }
    let id = id
        .as_ref()
        .map_err(|r| format!("пул {pool:#x} откатил getPoolId(): {r}"))?;
    P::getPoolIdCall::abi_decode_returns(id).map_err(|e| format!("getPoolId() не по ABI: {e}"))
}

/// Вызовы второго прохода: getPool(id) и getPoolTokens(id) хранилища, isPoolFromFactory фабрик. Ошибок нет.
pub fn second_calls(p: &Plan, id: B256) -> Vec<(Address, Bytes)> {
    let (pool, vault, wf, sf) = *p;
    let mut l: Vec<(Address, Bytes)> = vec![
        (vault, V::getPoolCall { poolId: id }.abi_encode().into()),
        (
            vault,
            V::getPoolTokensCall { poolId: id }.abi_encode().into(),
        ),
        (wf, F::isPoolFromFactoryCall { pool }.abi_encode().into()),
    ];
    if let Some(s) = sf {
        l.push((s, F::isPoolFromFactoryCall { pool }.abi_encode().into()));
    }
    l
}

/// Решение по ответам второго прохода (без сети). Принимает: кандидата, план, ответы по порядку second_calls.
/// Возвращает: найденный пул или причину отказа.
pub fn decide(c: &Candidate, p: Plan, a: &[Result<Bytes, String>]) -> Result<Found, String> {
    let (pool, _, _, sf) = p;
    let at = |i: usize| match a.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!(
            "хранилище или фабрика Balancer откатили вызов: {r}"
        )),
        None => Err("нет ответа хранилища / фабрики Balancer".to_string()),
    };
    let reg = V::getPoolCall::abi_decode_returns(at(0)?)
        .map_err(|e| format!("getPool не по ABI: {e}"))?;
    if reg._0 != pool {
        return Err(format!(
            "хранилище знает под id пула {pool:#x} другой адрес {:#x} — пул-самозванец",
            reg._0
        ));
    }
    let weighted = F::isPoolFromFactoryCall::abi_decode_returns(at(2)?)
        .map_err(|e| format!("isPoolFromFactory: {e}"))?;
    let stable = match sf {
        Some(_) => F::isPoolFromFactoryCall::abi_decode_returns(at(3)?)
            .map_err(|e| format!("isPoolFromFactory: {e}"))?,
        None => false,
    };
    if !weighted && !stable {
        let why = if sf.is_some() {
            ""
        } else {
            " (dex.balancer_stable_factory не задана — стейблсвап-пулы не берутся)"
        };
        return Err(format!(
            "пул {pool:#x} не создан фабриками DEX{why}: его код не сверен с исходником"
        ));
    }
    let pt = V::getPoolTokensCall::abi_decode_returns(at(1)?)
        .map_err(|e| format!("getPoolTokens не по ABI: {e}"))?;
    if !pt.tokens.contains(&c.coin) || !pt.tokens.contains(&c.quote) {
        return Err(format!(
            "среди токенов пула Balancer {pool:#x} нет пары {} / {}",
            c.coin, c.quote
        ));
    }
    let (token0, token1) = sorted(c.coin, c.quote);
    Ok(Found {
        family: c.family,
        reference: PoolRef::Contract(pool),
        token0,
        token1,
        coin: c.coin,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    })
}
