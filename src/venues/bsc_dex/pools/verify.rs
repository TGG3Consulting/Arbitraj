//! Файл: venues/bsc_dex/pools/verify.rs
//! Что делает: проверка пула из справочника индексатора (или из события создания) на своём узле перед тем, как его
//! слушать (решение 58; CLAUDE.md: «адрес контракта у биржи совпадает с адресом токена в пуле»). V2/V3: два прохода
//! Multicall3 — неизменяемые поля пула (factory, token0, token1; V3 — fee, tickSpacing и lmPool() — ABI PancakeSwap
//! или Uniswap; V2 со своей комиссией — метод dex.v2_fee_method), затем реестр фабрики DEX (getPair / getPool) —
//! именно он подтверждает, что это пул этой фабрики (поддельный контракт может вернуть из factory() что угодно).
//! V4/Infinity — verify_v4.rs; Ekubo — verify_ekubo.rs (решение 447). Вызовы — частями по dex.verify_batch: сбой узла на части — итог «не проверен» только
//! у её пулов (повтор — pools/index.rs), откат вызова — это ответ (пул отклонён с причиной).
//! V3: во второй проход — проба swapFee фабрики (решение 274, fee_probe.rs): фабрика берёт комиссию обмена, а
//! dex.v3_fee_source не задана — пулы DEX не берутся с причиной. V3 с dex.v3_registry_fee_method (решение 281,
//! SquadSwap V3): реестр спрашивается по значению её метода, ключ — в память (registry_key.rs), fee_pips — fee().
//! Кто вызывает: pools/index.rs, pools/created_log.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_check.rs, abi_v2v3.rs, discover.rs (Found), verify_rules.rs,
//! verify_calls.rs, verify_v4.rs, fee_probe.rs, registry_key.rs.
//! Настройки: dex.verify_batch, dex.multicall3 [bsc_dex]; dex.factory, dex.v2_fee_source, dex.v2_fee_method,
//! dex.v2_fee_denominator, dex.v3_fee_source, dex.v3_registry_fee_method [DEX].
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
use super::family_variant::Variant;
pub use super::verify_calls::{fee_method, field_calls, registry_call, registry_call_with};
pub use super::verify_rules::{Read, check, fill, registered, registered_for, v2_fee_pips, v3_abi};
use super::{
    fee_probe, verify_algebra, verify_balancer, verify_balancer_v3, verify_basin, verify_crypto,
    verify_crypto_v1, verify_curve, verify_curve_ng, verify_curve_plain, verify_dodo, verify_ekubo,
    verify_fluid, verify_inf_stable, verify_izi, verify_lb, verify_lb20, verify_lb22,
    verify_maverick, verify_metric, verify_solidly, verify_stable_u, verify_v4, verify_wombat,
};

/// Кандидат на проверку: DEX (вид), пул как у источника (адрес или PoolId), монета и встречный токен.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub family: &'static Family,
    pub pool: String,
    pub coin: Address,
    pub quote: Address,
}

/// Прочитанное с пула V2/V3 в первом проходе: адрес, поля и ключ реестра V3 (решение 281; None — без
/// dex.v3_registry_fee_method).
type Fields = (Address, Read, Option<u32>);

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
    let mut solidly = Vec::new();
    let mut curve = Vec::new();
    let mut curve_ng = Vec::new();
    let mut crypto = Vec::new();
    let mut crypto_v1 = Vec::new();
    let mut metric = Vec::new();
    let mut izi = Vec::new();
    let mut balancer = Vec::new();
    let mut balancer_v3 = Vec::new();
    let mut lb = Vec::new();
    let mut lb20 = Vec::new();
    let mut lb22 = Vec::new();
    let mut stable_u = Vec::new();
    let mut curve_plain = Vec::new();
    let mut mav = Vec::new();
    let mut inf_stable = Vec::new();
    let mut fluid = Vec::new();
    let mut basin = Vec::new();
    let mut shell = Vec::new();
    let mut ekubo = Vec::new();
    let mut wombat = Vec::new();
    let mut v2v3 = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        match c.family.proto {
            Proto::InfinityCl | Proto::InfinityBin | Proto::UniswapV4 => v4.push(i),
            p if p.is_dodo() => dodo.push(i),
            Proto::Algebra | Proto::AlgebraIntegral | Proto::AlgebraDirFee => algebra.push(i),
            p if p.is_solidly() => solidly.push(i),
            // Решение 472: Curve 2021 (curve_stableswap_plain) — список или реестр фабрики (verify_curve_plain.rs).
            // Решение 472 (доделка): метапул фабрики — тот же путь (реестр фабрики и get_base_pool).
            // Решение 481: пул с курсом оракула — тот же путь (список и oracle() пула).
            Proto::CurveStable
                if matches!(
                    c.family.variant,
                    Variant::CurvePlain | Variant::CurveMeta | Variant::CurveOracle
                ) =>
            {
                curve_plain.push(i)
            }
            // Решение 453: tricrypto 2021 — фабрики нет, как Curve 2020: список dex.stable_pools + coins(k).
            Proto::CurveStable | Proto::CurveTricryptoV1 => curve.push(i),
            Proto::CurveNg => curve_ng.push(i),
            Proto::CurveTwocrypto | Proto::CurveTricrypto => crypto.push(i),
            Proto::CurveCryptoV1 => crypto_v1.push(i),
            Proto::MetricOmm => metric.push(i),
            Proto::IziSwap | Proto::BiswapV3 => izi.push(i),
            Proto::BalancerV2 => balancer.push(i),
            Proto::BalancerV3 => balancer_v3.push(i),
            Proto::LbV21 => lb.push(i),
            Proto::LbV20 => lb20.push(i),
            // Решение 446: LB V2.2 — проверка V2.1 и ещё реализация и крючки пула (verify_lb22.rs).
            Proto::LbV22 => lb22.push(i),
            Proto::PancakeStable | Proto::ListaStable | Proto::SaddleStable | Proto::WooPP => {
                stable_u.push(i)
            }
            Proto::Maverick => mav.push(i),
            Proto::InfinityStable => inf_stable.push(i),
            Proto::FluidT1 => fluid.push(i),
            Proto::BasinWell => basin.push(i),
            Proto::ShellProteus => shell.push(i),
            Proto::Ekubo => ekubo.push(i),
            // Решение 484: Wombat — dex.stable_pools, getTokens пула, реализация прокси (verify_wombat.rs).
            Proto::Wombat => wombat.push(i),
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
    let list: Vec<Candidate> = solidly.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in solidly
        .into_iter()
        .zip(verify_solidly::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Curve StableSwap-NG (решение 275): реестр фабрики NG + монеты пула (verify_curve_ng.rs).
    let list: Vec<Candidate> = curve_ng.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in curve_ng
        .into_iter()
        .zip(verify_curve_ng::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Curve Twocrypto-NG / Tricrypto-NG (решение 442): реестр фабрики, версия, математика (verify_crypto.rs).
    let list: Vec<Candidate> = crypto.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in crypto
        .into_iter()
        .zip(verify_crypto::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Curve crypto v1 фабрики (решение 453): реестр фабрики и шаблон прокси пула (verify_crypto_v1.rs).
    let list: Vec<Candidate> = crypto_v1.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in crypto_v1
        .into_iter()
        .zip(verify_crypto_v1::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Metric OMM (решение 285): реестр фабрики isPool + getImmutables пула (verify_metric.rs).
    let list: Vec<Candidate> = metric.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in metric
        .into_iter()
        .zip(verify_metric::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // iZiSwap / Biswap V3 (решение 286): поля пула + реестр фабрики pool(x, y, fee) (verify_izi.rs).
    let list: Vec<Candidate> = izi.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in izi
        .into_iter()
        .zip(verify_izi::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Balancer V2 (решение 289): хранилище getPool(id) + фабрики isPoolFromFactory + токены пула (verify_balancer.rs).
    let list: Vec<Candidate> = balancer.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in balancer
        .into_iter()
        .zip(verify_balancer::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Balancer V3 (решение 441): хранилище знает пул + фабрики по выпускам + крючки обмена + токены (verify_balancer_v3.rs).
    let list: Vec<Candidate> = balancer_v3.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in balancer_v3
        .into_iter()
        .zip(verify_balancer_v3::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Liquidity Book V2.1 (решение 290): поля пула + реестр фабрики getLBPairInformation (verify_lb.rs).
    let list: Vec<Candidate> = lb.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in lb
        .into_iter()
        .zip(verify_lb::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Liquidity Book v2.0 (решение 445): factory / tokenX / tokenY / feeParameters пула и реестр фабрики (verify_lb20.rs).
    let list: Vec<Candidate> = lb20.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in lb20
        .into_iter()
        .zip(verify_lb20::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Liquidity Book V2.2 (решение 446): проверка V2.1 + реализация и крючки пула (verify_lb22.rs).
    let list: Vec<Candidate> = lb22.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in lb22
        .into_iter()
        .zip(verify_lb22::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // StableSwap (решение 122 F): список dex.stable_pools + монеты пула (verify_curve.rs).
    let list: Vec<Candidate> = curve.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in curve
        .into_iter()
        .zip(verify_curve::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Curve 2021 (решение 472): список dex.stable_pools или реестр фабрики + монеты пула (verify_curve_plain.rs).
    let list: Vec<Candidate> = curve_plain.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in curve_plain
        .into_iter()
        .zip(verify_curve_plain::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // PancakeSwap / Lista StableSwap, Saddle, WooFi (решение 122 F, 3): реестр фабрики / список / общий контракт.
    let list: Vec<Candidate> = stable_u.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in stable_u
        .into_iter()
        .zip(verify_stable_u::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Maverick V2 (решение 122 F): реестр фабрики isFactoryPool + токены + permissionedSwap (verify_maverick.rs).
    let list: Vec<Candidate> = mav.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in mav
        .into_iter()
        .zip(verify_maverick::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Infinity Stable (решение «122 (Infinity Stable)»): ключ у менеджера + крючок из dex.stable_hooks.
    let list: Vec<Candidate> = inf_stable.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in inf_stable
        .into_iter()
        .zip(verify_inf_stable::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Fluid DEX T1 (решение 440): isDex фабрики + constantsView пула (verify_fluid.rs).
    let list: Vec<Candidate> = fluid.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in fluid
        .into_iter()
        .zip(verify_fluid::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Скважина Basin (решение 485): реестр Aquifer / прокси, функция скважины, сверка calcLpTokenSupply
    // (verify_basin.rs).
    let list: Vec<Candidate> = basin.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in basin
        .into_iter()
        .zip(verify_basin::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Shell Protocol Proteus (решение 486): список dex.stable_pools, учёт dex.vault, стороны и реализация (verify_shell.rs).
    let list: Vec<Candidate> = shell.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in shell
        .into_iter()
        .zip(super::verify_shell::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    // Ekubo (решение 447): ключ у PoolKeyIndex + состояние у ядра (verify_ekubo.rs).
    let list: Vec<Candidate> = ekubo.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in ekubo
        .into_iter()
        .zip(verify_ekubo::verify(ctx, &list, size).await)
    {
        out[i] = Some(r);
    }
    let list: Vec<Candidate> = wombat.iter().map(|i| cands[*i].clone()).collect();
    for (i, r) in wombat
        .into_iter()
        .zip(verify_wombat::verify(ctx, &list).await)
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
    let mut first: Vec<Result<Fields, Verdict>> = Vec::new();
    let mut reg = Vec::new();
    for (c, plan) in cands.iter().zip(plans) {
        let r = match plan {
            Err(w) => Err(Verdict::Rejected(w)),
            Ok((a, at, n)) => match answers(&got[at..at + n]) {
                Err(node) => Err(Verdict::Failed(node)),
                Ok(list) => fields(ctx, c, &list)
                    .and_then(|read| Ok((a, read, super::registry_key::from_answers(&list)?)))
                    .map_err(Verdict::Rejected),
            },
        };
        if let Ok((_, read, key)) = &r {
            let factory = ctx.dex_addr("dex.factory", c.family.id).unwrap_or_default();
            reg.push((factory, registry_call_with(c.family, read, *key)));
        }
        first.push(r);
    }
    // Второй проход: реестр фабрики должен вернуть тот же адрес. В тот же проход — проба swapFee фабрик V3 (решение
    // 274, fee_probe.rs): один вызов на DEX при первой проверке её пулов, дальше ответ фабрики берётся из памяти.
    let (mut probes, asks) = fee_probe::plan(ctx, cands.iter().map(|c| &c.family));
    let n = reg.len();
    reg.extend(asks);
    let mut regs = call::try_chunks(ctx, &reg, "latest", size).await;
    probes.settle(&regs.split_off(n));
    let mut regs = regs.into_iter();
    let mut out = Vec::new();
    for (c, r) in cands.iter().zip(first) {
        out.push(match r {
            Err(v) => v,
            Ok((a, read, key)) => match regs.next() {
                None => Verdict::Failed("нет ответа реестра".into()),
                Some(Err(node)) => Verdict::Failed(node),
                Some(Ok(Err(rev))) => Verdict::Rejected(format!("реестр фабрики откатился: {rev}")),
                Some(Ok(Ok(b))) => match probes.check(c.family.id) {
                    Err(v) => v,
                    Ok(()) => match registered_for(c.family, a, &b)
                        .and_then(|_| found(ctx, c, a, &read, key))
                    {
                        Ok(f) => Verdict::Taken(Box::new(f)),
                        Err(w) => Verdict::Rejected(w),
                    },
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

/// Найденный пул V2/V3 по прочитанному (key — ключ реестра V3, решение 281). Ошибка — не задан делитель комиссии
/// или комиссия неверна.
fn found(
    ctx: &Ctx,
    c: &Candidate,
    pool: Address,
    r: &Read,
    key: Option<u32>,
) -> Result<Found, String> {
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
    // Решение 281: ключ реестра подтверждён реестром фабрики (registered) — в память для QuoterV2 (quoter_calls.rs).
    if let Some(k) = key {
        super::registry_key::remember(ctx.place.id, pool, k);
    }
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
