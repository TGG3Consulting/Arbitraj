//! Файл: venues/bsc_dex/pools/verify_balancer_v3.rs
//! Что делает: решение 441 (T433-F2) — проверка пула Balancer V3 из справочника на своём узле (решение 59). Кто
//! подтверждает пул: (1) хранилище dex.vault знает пул — getPoolTokenInfo(пул) (withRegisteredPool: незарегистрированный
//! адрес — откат) и отдаёт его токены; (2) пул создан фабрикой DEX — isPoolFromFactory фабрики из dex.bv3_*_factories
//! (выпуск фабрики = правило расчёта, balancer_v3_factories.rs); (3) крючок обмена пула — только официальный крючок
//! комиссии из списков DEX (решение 452, balancer_v3_hooks.rs: StableSurge, MEV Capture; beforeSwap / afterSwap и чужой
//! крючок — отказ); (4) монета и встречный
//! токен — среди токенов пула (CLAUDE.md: адрес токена у биржи = адрес в пуле). Один проход частями по dex.verify_batch.
//! Решение 474: задана dex.bv3_buffer_preview_raw — второй проход (verify_balancer_v3_buffer.rs): базовые токены
//! буферов ERC4626 токенов пула; монета пары может быть таким базовым токеном (обмен через буфер), и они же идут в
//! развёртку пар (multi_pair).
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_balancer_v3.rs, balancer_v3_factories.rs,
//! balancer_v3_hooks.rs, discover.rs, multi_pair.rs, verify.rs, verify_balancer_v3_buffer.rs.
//! Настройки: dex.vault, dex.bv3_*_factories, dex.bv3_stable_surge_hooks, dex.bv3_mev_capture_hooks [DEX вида
//! balancer_v3]; dex.bv3_buffer_preview_raw (решение 474).
//!
//! Источник: VaultExtension.sol (20241204-v3-vault) L520-L549 getPoolTokenInfo (withRegisteredPool), L585-L589
//! getHooksConfig; BasePoolFactory.sol L69-L72 isPoolFromFactory (только пулы, созданные этой фабрикой).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_balancer_v3::{IBalancerV3Factory as F, IBalancerV3Vault as V};
use super::balancer_v3_factories::{self as fac, Release};
use super::balancer_v3_hooks::{self, Lists};
use super::discover::{Found, sorted};
use super::multi_pair;
use super::verify::{Candidate, Verdict};
use super::verify_balancer_v3_buffer as buffer;

/// Адреса проверки: пул, хранилище, фабрики с выпуском, списки крючков (решение 452).
pub type Plan = (Address, Address, Vec<(Address, Release)>, Lists);

/// Первый проход: план и ответы его вызовов, либо готовый итог (отказ / сбой узла).
type First = Result<(Plan, Vec<Result<Bytes, String>>), Verdict>;

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (не задана настройка, пул не прошёл — отказ с
/// причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let plans: Vec<Result<Plan, String>> = cands.iter().map(|c| plan(ctx, c)).collect();
    let all: Vec<(Address, Bytes)> = plans.iter().flatten().flat_map(calls).collect();
    let got = call::try_chunks(ctx, &all, "latest", size).await;
    let mut got = got.into_iter();
    let mut first: Vec<First> = Vec::new();
    for p in plans {
        first.push(match p {
            Err(w) => Err(Verdict::Rejected(w)),
            Ok(p) => {
                let part: Vec<call::Piece> = got.by_ref().take(calls(&p).len()).collect();
                match part.into_iter().collect::<Result<Vec<_>, String>>() {
                    Err(node) => Err(Verdict::Failed(node)),
                    Ok(a) => Ok((p, a)),
                }
            }
        });
    }
    // Решение 474: базовые токены буферов ERC4626 токенов пула (второй проход; опорная сумма не задана — пусто).
    let lists: Vec<(Address, Vec<Address>)> = cands
        .iter()
        .zip(&first)
        .map(|(c, r)| match r {
            Ok((p, a)) if buffer::enabled(ctx, c.family.id) => (p.1, tokens(a)),
            _ => (Address::ZERO, Vec::new()),
        })
        .collect();
    let under = buffer::assets(ctx, &lists, size).await;
    let mut out = Vec::new();
    for ((c, r), u) in cands.iter().zip(first).zip(under) {
        out.push(match (r, u) {
            (Err(v), _) => v,
            (Ok(_), Err(node)) => Verdict::Failed(node),
            // Решение 474 (ревью t244): обёртка и её же базовый токен одного пула — пара не строится.
            (Ok(_), Ok(u)) if buffer::twin(&u, c.coin, c.quote) => Verdict::Rejected(format!(
                "{} / {} — обёртка ERC4626 и её базовый токен одного пула Balancer V3 (пул не нужен)",
                c.coin, c.quote
            )),
            (Ok((p, a)), Ok(u)) => {
                let under: Vec<Address> = u.iter().map(|x| x.1).collect();
                match decide(c, &p, &a, &under) {
                    Ok(f) => {
                        let mut tokens = tokens(&a);
                        for x in &under {
                            if !tokens.contains(x) {
                                tokens.push(*x);
                            }
                        }
                        buffer::remember(ctx, p.0, &u);
                        Verdict::Taken(Box::new(multi_pair::adopt(ctx, f, &tokens)))
                    }
                    Err(w) => Verdict::Rejected(w),
                }
            }
        });
    }
    out
}

/// Адреса проверки кандидата. Возвращает: план или причину отказа (не адрес, не задана dex.vault / dex.bv3_*).
fn plan(ctx: &Ctx, c: &Candidate) -> Result<Plan, String> {
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта пула Balancer V3", c.pool))?;
    let vault = ctx
        .dex_addr("dex.vault", c.family.id)
        .map_err(|e| e.message)?;
    let (list, hooks) = super::puzzle_ranges::bindings(ctx, c.family)?;
    Ok((pool, vault, list, hooks))
}

/// Вызовы проверки: getPoolTokenInfo и getHooksConfig хранилища, isPoolFromFactory каждой фабрики. Ошибок нет.
pub fn calls(p: &Plan) -> Vec<(Address, Bytes)> {
    let (pool, vault, list, _) = p;
    let pool = *pool;
    let mut l: Vec<(Address, Bytes)> = vec![
        (*vault, V::getPoolTokenInfoCall { pool }.abi_encode().into()),
        (*vault, V::getHooksConfigCall { pool }.abi_encode().into()),
    ];
    for (f, _) in list {
        l.push((*f, F::isPoolFromFactoryCall { pool }.abi_encode().into()));
    }
    l
}

/// Решение по ответам (без сети). Принимает: кандидата, план, ответы по порядку calls, базовые токены буферов ERC4626
/// токенов пула (решение 474; пусто — буферов нет или не читались). Возвращает: найденный пул или причину отказа.
pub fn decide(
    c: &Candidate,
    p: &Plan,
    a: &[Result<Bytes, String>],
    under: &[Address],
) -> Result<Found, String> {
    let (pool, _, list, hooks_list) = p;
    let at = |i: usize| match a.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!(
            "хранилище или фабрика Balancer V3 откатили вызов {i}: {r}"
        )),
        None => Err("нет ответа хранилища / фабрики Balancer V3".to_string()),
    };
    let info = V::getPoolTokenInfoCall::abi_decode_returns(at(0)?)
        .map_err(|e| format!("getPoolTokenInfo не по ABI: {e}"))?;
    let hooks = V::getHooksConfigCall::abi_decode_returns(at(1)?)
        .map_err(|e| format!("getHooksConfig не по ABI: {e}"))?;
    let mut yes = Vec::with_capacity(list.len());
    for i in 0..list.len() {
        let r = F::isPoolFromFactoryCall::abi_decode_returns(at(2 + i)?)
            .map_err(|e| format!("isPoolFromFactory: {e}"))?;
        yes.push(r);
    }
    let release = fac::release(list, &yes).map_err(|w| format!("пул {pool:#x}: {w}"))?;
    // Решение 452: крючки StableSurge / MEV Capture из списков DEX берутся, прочие крючки обмена — нет.
    balancer_v3_hooks::kind(hooks_list, &hooks, release)
        .map_err(|w| format!("пул {pool:#x}: {w}"))?;
    let has = |t: &Address| info.tokens.contains(t) || under.contains(t);
    if !has(&c.coin) || !has(&c.quote) {
        return Err(format!(
            "среди токенов пула Balancer V3 {pool:#x} и базовых токенов их буферов нет пары {} / {}",
            c.coin, c.quote
        ));
    }
    let (token0, token1) = sorted(c.coin, c.quote);
    Ok(Found {
        family: c.family,
        reference: PoolRef::Contract(*pool),
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

/// Токены пула по ответу getPoolTokenInfo (решение 295, развёртка пар). Ответ не по ABI — пусто. Ошибок нет.
fn tokens(answers: &[Result<Bytes, String>]) -> Vec<Address> {
    answers
        .first()
        .and_then(|a| a.as_ref().ok())
        .and_then(|b| V::getPoolTokenInfoCall::abi_decode_returns(b).ok())
        .map(|i| i.tokens)
        .unwrap_or_default()
}
