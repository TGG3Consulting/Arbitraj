//! Файл: venues/bsc_dex/pools/solidly_refresh.rs
//! Что делает: решение 273 — перечитывание комиссии и ограничений обмена (пауза фабрики, предел цены газа) пулов
//! Solidly, у DEX которых задан источник комиссии (dex.solidly_fee_source) или методы ограничений. Эти значения
//! меняются без события пула (Sync их не несёт) — перечитываются тем же путём, что своя комиссия пар V2
//! (pools/index_watch.rs refresh_fees): перед каждым проходом сверки с квотером (check/quoter_cycle.rs), в реестр.
//! У DEX без этих настроек (THENA V1, Topaz и др.) ничего не перечитывается — как до решения 273.
//! Ревью 273 (п. 4): ограничения фабрики одинаковы для всех её пулов — читаются ОДИН раз на DEX за проход; комиссии
//! пулов — пачкой (node/call.rs try_chunks: Multicall3 при dex.multicall3, иначе пачка JSON-RPC) частями по
//! dex.verify_batch — как проверка и перечитывание комиссии пар V2 (pools/verify.rs batch). Сбой узла на части —
//! только пулы этой части остаются с прежним состоянием (запись в журнал).
//! Решение 470: пулы Aero Basic — состояние заново (load_aero_basic.rs) тем же проходом.
//! Кто вызывает: check/quoter_cycle.rs.
//! От чего зависит: pools/{listen, registry, load_solidly, solidly_fee_read, verify}.rs, node/call.rs (try_chunks),
//! core/types (PoolState, PoolRef).
//! Настройки: dex.factory, dex.solidly_fee_source, dex.solidly_pause_method, dex.solidly_max_gas_method [DEX],
//! dex.verify_batch [bsc_dex]; через solidly_fee_read.rs — остальные dex.solidly_fee_* [DEX].

use alloy_primitives::{Address, Bytes};

use crate::core::types::{FeeRule, PoolRef, PoolState, SolidlyFee};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::node::call;
use super::families::{Family, Proto};
use super::listen::Pools;
use super::{load_aero_basic, load_solidly, solidly_fee_read as read, verify};

/// Пул Solidly к перечитыванию: ссылка, адрес, флаг stable.
type Item = (PoolRef, Address, bool);

/// Перечитывает комиссию и ограничения обмена пулов Solidly (см. шапку) на последнем блоке узла. Ошибок нет: сбой —
/// в журнал, состояние пула прежнее (сверка с квотером на блоке всё равно читает пул заново, check/quoter_block.rs).
pub async fn refresh(p: &Pools) {
    // Пулы по DEX (порядок первого появления — как у прохода реестра).
    let mut by: Vec<(&'static Family, Vec<Item>)> = Vec::new();
    let mut aero: Vec<(PoolRef, &'static Family)> = Vec::new();
    for r in p.reg.refs() {
        let Some(e) = p.reg.get(&r) else { continue };
        // Решение 470: пул Aero Basic — комиссия getBaseFee и пауза фабрики меняются без событий пула: перечитать.
        if e.family.proto == Proto::AeroBasic {
            aero.push((r.clone(), e.family));
            continue;
        }
        // Решение 443: пары Camelot V2 (тоже PoolState::Solidly) — не Solidly: комиссии у них из события пары.
        if !e.family.proto.is_solidly()
            || e.family.variant == super::family_variant::Variant::LeetSwap
        {
            continue;
        }
        let (PoolState::Solidly { stable, .. }, PoolRef::Contract(pool)) = (&e.pool.state, &r)
        else {
            continue;
        };
        let item = (r.clone(), *pool, *stable);
        match by.iter_mut().find(|(f, _)| f.id == e.family.id) {
            Some((_, list)) => list.push(item),
            None => by.push((e.family, vec![item])),
        }
    }
    for (r, f) in aero {
        aero_basic(p, &r, f).await;
    }
    for (f, items) in by {
        if let Err(err) = family(p, f, &items).await {
            let msg = format!(
                "{}: комиссия/ограничения пулов Solidly не перечитаны: {}",
                f.id, err.message
            );
            p.ctx.log(Level::Warn, "dex.index", msg, None).await;
        }
    }
}

/// Одна DEX: настройки, ограничения фабрики (один раз), комиссии пулов пачкой, запись в реестр. Ошибки: хранилище
/// настроек, NotConfigured (фабрика, метод/правило источника, dex.verify_batch), узел и BadResponse ограничений —
/// пулы DEX остаются с прежним состоянием. Сбой комиссии отдельного пула — в журнал, остальные обновляются.
async fn family(p: &Pools, f: &'static Family, items: &[Item]) -> GatewayResult<()> {
    let ctx = &p.ctx;
    let src = read::source(ctx, f)?;
    if src.is_none() && !read::has_gate(ctx, f)? {
        return Ok(());
    }
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let rule = read::rule(ctx, f, src.as_deref())?;
    let calls: Vec<(Address, Bytes)> = items
        .iter()
        .map(|(_, pool, stable)| read::fee_call(ctx, f, src.as_deref(), (factory, *pool, *stable)))
        .collect::<GatewayResult<_>>()?;
    let size = verify::batch(ctx)?;
    let gate = read::gate(ctx, f, factory, "latest").await?;
    let got = call::try_chunks(ctx, &calls, "latest", size).await;
    for ((r, pool, _), ((to, _), answer)) in items.iter().zip(calls.iter().zip(got)) {
        let fee = match answer {
            Ok(Ok(bytes)) => value(p, src.is_some(), *to, &bytes, rule),
            Ok(Err(revert)) => Err(format!("откат: {revert}")),
            Err(node) => Err(node),
        };
        match fee {
            Ok(nf) => {
                p.reg.update(r, |x| {
                    if let PoolState::Solidly { fee, gate: g, .. } = &mut x.pool.state {
                        *fee = nf;
                        *g = gate;
                    }
                });
            }
            Err(why) => {
                let msg = format!(
                    "{} {pool:#x}: комиссия/ограничения пула не перечитаны: {why}",
                    f.id
                );
                ctx.log(Level::Warn, "dex.index", msg, None).await;
            }
        }
    }
    Ok(())
}

/// Ответ метода комиссии → комиссия с правилом: источник задан — проверка по правилу (solidly_fee_read.rs checked);
/// не задан — как до решения 273 (load_solidly.rs fee_u32: числитель над 10000). Ошибка — текст.
fn value(
    p: &Pools,
    has_source: bool,
    to: Address,
    bytes: &[u8],
    rule: FeeRule,
) -> Result<SolidlyFee, String> {
    let ctx = &p.ctx;
    let raw = read::word(ctx, to, bytes).map_err(|e| e.message)?;
    if has_source {
        return read::checked(ctx, raw, rule).map_err(|e| e.message);
    }
    let value = load_solidly::fee_u32(ctx, raw).map_err(|e| e.message)?;
    Ok(SolidlyFee { value, rule })
}

/// Решение 470: пул Aero Basic — состояние заново на последнем блоке (load_aero_basic.rs: комиссия, пауза, правило
/// MEV-налога) в реестр. Ошибок нет: сбой — в журнал, состояние прежнее (перед отправкой пауза и налог проверяются
/// заново — exec/solidly_gate.rs fresh_cap).
async fn aero_basic(p: &Pools, r: &PoolRef, f: &'static Family) {
    let PoolRef::Contract(pool) = r else { return };
    match load_aero_basic::load(&p.ctx, f, *pool, "latest").await {
        Ok(st) => {
            p.reg.update(r, |x| x.pool.state = st);
        }
        Err(err) => {
            let msg = format!(
                "{} {pool:#x}: пул Aero Basic не перечитан: {}",
                f.id, err.message
            );
            p.ctx.log(Level::Warn, "dex.index", msg, None).await;
        }
    }
}
