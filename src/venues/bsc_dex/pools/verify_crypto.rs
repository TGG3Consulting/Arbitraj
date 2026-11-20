//! Файл: venues/bsc_dex/pools/verify_crypto.rs
//! Что делает: проверка crypto-пула Curve (решение 442: Twocrypto-NG, Tricrypto-NG) из справочника или поиска по
//! фабрике на своём узле. Кто подтверждает пул (решение 59) — реестр фабрики dex.factory: `get_coins(pool)` отдаёт
//! монеты, только если пул развёрнут этой фабрикой (pool_data чужого адреса пуст — нулевые адреса). Пул берётся, если:
//! монеты из реестра не нулевые; монета и встречный токен — среди них (CLAUDE.md: адрес токена у биржи = адрес в пуле);
//! `version()` пула — в списке dex.curve_crypto_versions (у фабрики несколько реализаций пула, pool_implementations);
//! `MATH()` пула — в списке dex.curve_crypto_math (адреса контрактов математики, с исходником которых сверена
//! math/crypto_y2.rs / crypto_y3.rs: пул зовёт свою математику, у разных версий она разная; решение 453 — и адреса
//! dex.curve_crypto_math_v2_0_0, математика Twocrypto-NG v2.0.0). Иначе — отказ с причиной.
//! Ревью 453: версия формул выбирается уже здесь (crypto_variant::variant): адрес MATH() сразу в обоих списках или
//! математика v2.0.0 у трёх монет — отказ с причиной; списки — crypto_variant::lists (ошибка разбора не гасится).
//! Вызовы — частями по dex.verify_batch.
//! Кто вызывает: pools/verify.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_crypto.rs, discover.rs, multi_pair.rs, verify.rs.
//! Настройки: dex.factory, dex.curve_crypto_versions, dex.curve_crypto_math, dex.curve_crypto_math_v2_0_0 [DEX].
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb, CurveTwocryptoFactory.vy get_coins L404-L410 (`self.pool_data[_pool].coins`);
//! curvefi/tricrypto-ng ecaa816, CurveL2TricryptoFactory.vy get_coins L387-L393; version и MATH — abi_crypto.rs.

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_crypto::{ICryptoPool as P, ITricryptoFactory as F3, ITwocryptoFactory as F2};
use super::crypto_variant;
use super::discover::{Found, sorted};
use super::families::Proto;
use super::multi_pair;
use super::verify::{Candidate, Verdict};

/// Вызовов на кандидата: get_coins (фабрика), version, MATH (пул).
const PER: usize = 3;

/// Что нужно для решения по кандидату: пул, фабрика, версии, адреса математики v2.0.0 и прочие (решение 453).
struct Plan {
    pool: Address,
    factory: Address,
    versions: Vec<String>,
    v200: Vec<Address>,
    ng: Vec<Address>,
}

/// Проверка пачки кандидатов. Возвращает: итог по каждому. Ошибок нет (нет настройки или пул не из реестра — отказ с
/// причиной; сбой узла — Failed).
pub async fn verify(ctx: &Ctx, cands: &[Candidate], size: usize) -> Vec<Verdict> {
    let mut plans: Vec<Result<Plan, String>> = Vec::new();
    let mut calls: Vec<(Address, Bytes)> = Vec::new();
    for c in cands {
        let plan = plan(ctx, c);
        if let Ok(p) = &plan {
            let pool = p.pool;
            let coins: Bytes = if c.family.proto == Proto::CurveTwocrypto {
                F2::get_coinsCall { pool }.abi_encode().into()
            } else {
                F3::get_coinsCall { pool }.abi_encode().into()
            };
            calls.push((p.factory, coins));
            calls.push((pool, P::versionCall {}.abi_encode().into()));
            calls.push((pool, P::MATHCall {}.abi_encode().into()));
        }
        plans.push(plan);
    }
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    let mut got = got.into_iter();
    let mut out = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let plan = match plan {
            Ok(p) => p,
            Err(w) => {
                out.push(Verdict::Rejected(w));
                continue;
            }
        };
        let part: Vec<call::Piece> = got.by_ref().take(PER).collect();
        let answers: Result<Vec<_>, String> = part.into_iter().collect();
        out.push(match answers {
            Err(node) => Verdict::Failed(node),
            Ok(a) => judge(ctx, c, &plan, &a),
        });
    }
    out
}

/// Пул кандидата и настройки DEX. Возвращает: Plan или причину отказа (не адрес, нет dex.factory, списки версий или
/// математики не заданы, пусты, хранятся не списком или с неверным адресом). Ошибок нет.
fn plan(ctx: &Ctx, c: &Candidate) -> Result<Plan, String> {
    let id = c.family.id;
    let pool = Address::from_str(&c.pool)
        .map_err(|_| format!("«{}» — не адрес контракта crypto-пула Curve", c.pool))?;
    let factory = ctx.dex_addr("dex.factory", id).map_err(|e| e.message)?;
    let not_taken = |what: String| format!("crypto-пулы Curve не берутся: {what}");
    let versions = ctx
        .dex_list("dex.curve_crypto_versions", id)
        .map_err(|e| not_taken(e.message))?;
    // Решение 453 (ревью): dex.curve_crypto_math_v2_0_0 не задана — пустой список (пулы v2.0.0 не берутся,
    // docs/SETTINGS_CASES.md); хранится не списком или не адрес — отказ с причиной (crypto_variant::lists).
    let (v200, ng) = crypto_variant::lists(ctx, id).map_err(|e| not_taken(e.message))?;
    if versions.is_empty() || (v200.is_empty() && ng.is_empty()) {
        let what = format!(
            "dex.curve_crypto_versions / dex.curve_crypto_math [dex:{id}] пуст"
        );
        return Err(not_taken(what));
    }
    Ok(Plan {
        pool,
        factory,
        versions,
        v200,
        ng,
    })
}

/// Монеты пула из ответа get_coins фабрики (по виду). Принимает: вид (Twocrypto ли), байты. Возвращает: монеты или
/// текст «не по ABI». Ошибок нет.
fn coins_of(two: bool, b: &[u8]) -> Result<Vec<Address>, String> {
    let r = if two {
        F2::get_coinsCall::abi_decode_returns(b).map(|c| c.to_vec())
    } else {
        F3::get_coinsCall::abi_decode_returns(b).map(|c| c.to_vec())
    };
    r.map_err(|e| format!("get_coins не по ABI: {e}"))
}

/// Решение по ответам (без сети): get_coins фабрики, version() и MATH() пула. Принимает: кандидата, план, ответы по
/// порядку (откат вызова — Err с текстом). Возвращает: (найденный пул, монеты пула) или причину отказа.
fn decide(
    c: &Candidate,
    p: &Plan,
    answers: &[Result<Bytes, String>],
) -> Result<(Found, Vec<Address>), String> {
    let pool = p.pool;
    let id = c.family.id;
    let at = |i: usize| match answers.get(i) {
        Some(Ok(b)) => Ok(b),
        Some(Err(r)) => Err(format!(
            "реестр фабрики Curve или пул {pool:#x} откатил вызов: {r}"
        )),
        None => Err("реестр фабрики Curve: нет ответа".to_string()),
    };
    let coins = coins_of(c.family.proto == Proto::CurveTwocrypto, at(0)?)?;
    if coins.iter().any(|a| a.is_zero()) {
        return Err(format!(
            "пул {pool:#x} не из реестра фабрики crypto-пулов Curve (get_coins пуст) — dex.factory DEX {id}"
        ));
    }
    let version = P::versionCall::abi_decode_returns(at(1)?)
        .map_err(|e| format!("version() пула {pool:#x} не по ABI: {e}"))?;
    if !p.versions.contains(&version) {
        return Err(format!(
            "пул {pool:#x} — реализация версии «{version}», её нет в dex.curve_crypto_versions [dex:{id}] ({})",
            p.versions.join(", ")
        ));
    }
    let math = P::MATHCall::abi_decode_returns(at(2)?)
        .map_err(|e| format!("MATH() пула {pool:#x} не по ABI: {e}"))?;
    // Решение 453 (ревью): версия формул по MATH() — как при чтении (load_crypto.rs state); неоднозначный адрес (в
    // обоих списках), v2.0.0 у трёх монет, адреса нет в списках — отказ с причиной.
    if let Err(w) = crypto_variant::variant(coins.len(), math, &p.v200, &p.ng) {
        return Err(format!("пул {pool:#x} [dex:{id}]: {w}"));
    }
    if !coins.contains(&c.coin) || !coins.contains(&c.quote) {
        return Err(format!(
            "среди монет crypto-пула Curve {pool:#x} нет пары {} / {}",
            c.coin, c.quote
        ));
    }
    let (token0, token1) = sorted(c.coin, c.quote);
    let found = Found {
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
    };
    Ok((found, coins))
}

/// Что делает: итог по ответам проверки (без сети): decide и — решение 295 — у Tricrypto в режиме dex.multi_pair
/// ссылка с парой и монеты пула для развёртки (multi_pair::adopt; у Twocrypto режим не действует). Принимает:
/// окружение, кандидата, план, ответы. Возвращает: итог. Ошибок нет.
fn judge(ctx: &Ctx, c: &Candidate, p: &Plan, answers: &[Result<Bytes, String>]) -> Verdict {
    match decide(c, p, answers) {
        Ok((f, coins)) => Verdict::Taken(Box::new(multi_pair::adopt(ctx, f, &coins))),
        Err(w) => Verdict::Rejected(w),
    }
}
