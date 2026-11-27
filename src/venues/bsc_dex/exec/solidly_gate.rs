//! Файл: venues/bsc_dex/exec/solidly_gate.rs
//! Что делает: решение 273 — ограничения обмена пар Solidly, которые проверяет сам контракт пары (usdfi BasePair:
//! `require(!factory.isPaused())`, `require(tx.gasprice <= factory.maxGasPrice())`), на стороне плана и отправки:
//! - planned — по состоянию пулов в памяти (gate из load_solidly.rs / solidly_refresh.rs) и цене газа плана: причина,
//!   почему маршрут/цикл неисполним (котировка, исполнимость);
//! - fresh_cap — перед отправкой: ограничения заново с фабрики на последнем блоке (только DEX с заданными методами);
//!   пауза — сделки нет; возвращает наименьший предел цены газа — его проверяет exec/txs.rs send_capped по цене самой
//!   транзакции (legacy gasPrice; чаевые канала — отдельным переводом, на gasPrice свопа не влияют); свежие
//!   ограничения, отличные от памяти, — в реестр всем пулам DEX (ревью 273, п. 3).
//!
//! Решение 470: у Aero Slipstream V3 — pools/aero_cl.rs fee (крючок, MEV-налог); у Aero Basic перед отправкой — pools/aero_mev.rs fresh (пауза фабрики, правило MEV-налога).
//! Кто вызывает: quote/cycle_cost.rs, quote/mod.rs (planned), exec/executor/trade.rs, exec/cycle_exec.rs (fresh_cap).
//! От чего зависит: math/solidly_fee.rs (blocked), pools/{registry, solidly_fee_read}.rs, quote/mod.rs (Quoting),
//! core/types (PoolState, SolidlyGate).
//! Настройки: через solidly_fee_read.rs — dex.factory, dex.solidly_pause_method, dex.solidly_max_gas_method [DEX].

use alloy_primitives::{Address, U256};

use crate::core::types::{PoolRef, PoolState, SolidlyGate};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::solidly_fee;
use super::super::pools::aero_cl;
use super::super::pools::aero_mev;
use super::super::pools::exec_kind::{self, ExecKind};
use super::super::pools::families::Proto;
use super::super::pools::family_variant::Variant;
use super::super::pools::registry::{Entry, Registry};
use super::super::pools::solidly_fee_read;
use super::super::quote::Quoting;

/// Причина, почему обмен через один из пулов откатится при цене газа плана. Принимает: пулы шагов, цену газа.
/// Возвращает: «DEX пул: причина» первого запрещающего пула или None. Ошибок нет.
pub fn reason<'a>(entries: impl IntoIterator<Item = &'a Entry>, price: U256) -> Option<String> {
    entries.into_iter().find_map(|e| match &e.pool.state {
        PoolState::Solidly { gate, .. } => solidly_fee::blocked(gate, Some(price))
            .map(|w| format!("{} {:?}: {w}", e.family.id, e.pool.reference)),
        _ => None,
    })
}

/// То же, что reason, ошибкой. Ошибки: NoRoute — с точной причиной.
pub fn planned<'a>(
    ctx: &Ctx,
    entries: impl IntoIterator<Item = &'a Entry>,
    price: U256,
) -> GatewayResult<()> {
    match reason(entries, price) {
        Some(w) => Err(ctx.err(ErrorCode::NoRoute, w)),
        None => Ok(()),
    }
}

/// Перед отправкой: ограничения фабрик пулов Solidly шагов заново (последний блок), только у DEX с заданными методами.
/// Ревью 273 (п. 3): свежие ограничения отличаются от памяти — записываются в реестр всем пулам этой DEX (remember),
/// чтобы до следующего прохода сверки котировка не выбирала пул на паузе / с пределом ниже плановой цены газа.
/// Принимает: котировку (окружение и реестр), пулы шагов. Возвращает: наименьший предел цены газа (None — пределов нет).
/// Ошибки: NoRoute — фабрика на паузе; узел, BadResponse, NotConfigured (dex.factory), хранилище настроек (has_gate).
pub async fn fresh_cap(q: &Quoting, entries: &[&Entry]) -> GatewayResult<Option<U256>> {
    let ctx = &q.ctx;
    if !super::super::pools::nami::path_ok(entries.iter().copied()) {
        return Err(ctx.err(ErrorCode::NoRoute, "Nami разрешает только отдельный обмен"));
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut cap: Option<U256> = None;
    let mut role_block = None;
    for e in entries.iter().copied() {
        let f = e.family;
        if f.variant==Variant::IziModules{
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::izi_modules::at(ctx,f,e.pool.reference.address(),(e.pool.token0,e.pool.token1),at)).await?;
        }
        if f.variant==Variant::AlgebraSingleV19 {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::synth_farming::check(ctx,f,e.pool.reference.address(),at)).await?;
            continue;
        }
        if f.variant==Variant::DeliCl && let PoolRef::Singleton{manager,id}=e.pool.reference {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::deli_cl::check(ctx,f,manager,id,(e.pool.token0,e.pool.token1),at)).await?;
            continue;
        }
        if f.variant==Variant::DeliConstant && let PoolRef::Singleton{manager,id}=e.pool.reference {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::deli::market(ctx,f,manager,id,(e.pool.token0,e.pool.token1),at)).await?;
            continue;
        }
        if f.variant == Variant::ElasticV5 {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::elastic_binding::check(ctx,f,e.pool.reference.address(),at)).await?;
            if f.tradable {super::super::pools::direct_module::verify_code(ctx,f.id,ctx.dex_addr("dex.router",f.id)?,at).await?;}
            continue;
        }
        if matches!(f.variant,Variant::BasePluginV1|Variant::TrebleIntegral) {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            let fee=Box::pin(super::super::pools::base_plugin_v1::check(ctx,f,e.pool.reference.address(),at)).await?;
            if !matches!(&e.pool.state,PoolState::Algebra{fee:old,..} if *old==u32::from(fee)) {
                return Err(ctx.err(ErrorCode::NoRoute,"Algebra BasePluginV1: ставка изменилась после котировки"));
            }
            continue;
        }
        if f.variant.solidly_classic() {
            let at = match &role_block {
                Some(at) => at,
                None => role_block.insert(super::super::pools::nami::block(ctx, "latest").await?),
            };
            let gate = super::super::pools::solidly_classic::fresh(ctx,e,at).await?;
            cap = merged_cap(ctx, f.id, gate, cap)?;
            continue;
        }
        if f.variant==Variant::Temple {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::temple::market(ctx,f,e.pool.reference.address(),(e.pool.token0,e.pool.token1),at)).await?;
            continue;
        }
        if matches!(f.variant, Variant::RegisteredV3 | Variant::HelloV3) {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::registered_v3::fresh(ctx,e,at)).await?;
            continue;
        }
        if f.variant == Variant::Virtuals {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::virtuals::fresh(ctx,e,at)).await?;
            if f.tradable {super::super::pools::direct_module::verify_code(ctx,f.id,ctx.dex_addr("dex.router",f.id)?,at).await?;}
            continue;
        }
        if f.variant == Variant::BunnySwap {
            let at=match &role_block {Some(at)=>at,None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?)};
            Box::pin(super::super::pools::bunnyswap::fresh(ctx,e,at)).await?;
            Box::pin(super::super::pools::direct_module::verify_factory(ctx,f,ctx.dex_addr("dex.router",f.id)?,at)).await?;
            continue;
        }
        if f.variant == Variant::Memebox {
            let at=match &role_block {
                Some(at)=>at,
                None=>role_block.insert(super::super::pools::nami::block(ctx,"latest").await?),
            };
            Box::pin(super::super::pools::memebox::fresh(ctx,e,at)).await?;
            continue;
        }
        if f.variant == Variant::CandySwap {
            let at = match &role_block {
                Some(at) => at,
                None => role_block.insert(super::super::pools::nami::block(ctx, "latest").await?),
            };
            super::super::pools::v2_live_fee::candy_fresh(ctx,e,at).await?;
            if f.tradable { super::super::pools::direct_module::verify_factory(ctx,f,ctx.dex_addr("dex.router",f.id)?,at).await?; }
            continue;
        }
        if f.variant == Variant::V2DualCreate2LiveFee {
            let at = match &role_block {
                Some(at) => at,
                None => role_block.insert(super::super::pools::nami::block(ctx, "latest").await?),
            };
            super::super::pools::v2_pair_binding::fresh(ctx,e,at).await?;
            continue;
        }
        if f.variant.diamond_router() {
            let at = match &role_block {
                Some(at) => at,
                None => role_block.insert(super::super::pools::nami::block(ctx, "latest").await?),
            };
            if f.variant==Variant::KewlV2 {
                super::super::pools::kewl::checked(ctx,e,false,at).await?;
            } else {
                super::super::pools::diamond::fresh(ctx,e,at).await?;
            }
            continue;
        }
        if f.variant == Variant::LeetSwap {
            let PoolRef::Contract(pool) = e.pool.reference else {
                return Err(ctx.err(ErrorCode::NoRoute, "У LeetSwap нет адреса пары"));
            };
            super::super::pools::leetswap::fresh(ctx, f, pool, &e.pool.state).await?;
            continue;
        }
        if f.variant == Variant::Oasis {
            let (wallet, _, adapter) = super::oasis_direct::chosen(e).unwrap_or((
                super::super::wallet::signer::address(ctx)?,
                false,
                true,
            ));
            let h = super::super::quote::white_state::head(ctx, "latest").await?;
            Box::pin(super::super::quote::oasis_pricing::state(
                ctx, e, wallet, h, adapter,
            ))
            .await?;
            continue;
        }
        if f.variant == Variant::FraxTwamm {
            let pool = e
                .pool
                .reference
                .own_contract()
                .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Fraxswap: нет адреса пары"))?;
            Box::pin(super::super::pools::fraxswap::check_adapter(
                ctx,
                f,
                pool,
                (e.pool.token0, e.pool.token1),
                "latest",
            ))
            .await?;
            continue;
        }
        if f.direct_module() && f.tradable {
            let module = ctx.dex_addr("dex.router", f.id)?;
            super::super::pools::direct_module::verify(ctx, f, module, "latest").await?;
            continue;
        }
        if f.variant.external_algebra_swap()
            && let PoolRef::Contract(pool) = e.pool.reference
        {
            let hash = super::super::pools::nami::block(ctx, "latest").await?;
            let fee = if f.variant==Variant::SupernovaIntegral {
                Box::pin(super::super::pools::supernova::check(ctx,f,pool,&hash)).await?.0
            } else { Box::pin(super::super::pools::nami::check(ctx, f.id, pool, &hash)).await? };
            if !matches!(&e.pool.state,crate::core::types::PoolState::Algebra {fee:quoted, ..} if *quoted==u32::from(fee))
            {
                return Err(ctx.err(ErrorCode::StaleData, "Изменилась комиссия отправителя Algebra Integral"));
            }
            continue;
        }
        if f.variant == Variant::QuickswapIntegral
            && let PoolRef::Contract(pool) = e.pool.reference
        {
            super::super::pools::quickswap_plugin::check(ctx, f.id, pool, "latest").await?;
            continue;
        }
        // Решение 470: пул Aero Slipstream V3 — крючок, модуль и правило MEV-налога и комиссия заново (aero_fresh).
        if matches!(
            f.variant,
            Variant::SlipstreamAero
                | Variant::SlipstreamAeroMetaquoter
                | Variant::SlipstreamDynamicFee
        ) && let PoolRef::Contract(pool) = e.pool.reference
        {
            aero_fresh(q, e, pool).await?;
            continue;
        }
        if !matches!(e.pool.state, PoolState::Solidly { .. }) || seen.contains(&f.id) {
            continue;
        }
        seen.push(f.id);
        // Решение 470: Aero Basic — пауза фабрики и правило MEV-налога (pools/aero_mev.rs) заново перед отправкой;
        // T470-06: у исполнителя v5 (шаг AERO_BASIC) налог исполняется — проверяется только пауза.
        if f.proto == Proto::AeroBasic {
            let taxed_ok = exec_kind::for_wallet(ctx, f) == Some(ExecKind::AeroBasic);
            aero_mev::fresh(ctx, f.id, ctx.dex_addr("dex.factory", f.id)?, taxed_ok).await?;
            continue;
        }
        if !solidly_fee_read::has_gate(ctx, f)? {
            continue;
        }
        let factory = ctx.dex_addr("dex.factory", f.id)?;
        let g = solidly_fee_read::gate(ctx, f, factory, "latest").await?;
        remember(&q.reg, entries, f.id, g);
        cap = merged_cap(ctx, f.id, g, cap)?;
    }
    Ok(cap)
}

fn merged_cap(ctx: &Ctx, id: &str, gate: SolidlyGate, cap: Option<U256>) -> GatewayResult<Option<U256>> {
    if let Some(why) = solidly_fee::blocked(&gate, None) {
        return Err(ctx.err(ErrorCode::NoRoute, format!("{id}: {why} — своп не отправлен")));
    }
    Ok(match gate.max_gas_price.map(U256::from) {
        Some(limit) => Some(cap.map_or(limit, |c| c.min(limit))),
        None => cap,
    })
}

/// Решение 470 (ревью t240): пул Aero Slipstream V3 перед отправкой — комиссия на последнем блоке (aero_cl::fee_latest:
/// крючок пула, закреплённый модуль MEV-налога, первая комиссия следующего блока) против комиссии записи, по которой
/// посчитана котировка. Выросла — отказ: выход сделки был бы ниже посчитанного. Реестр здесь не пишется (ревью t240):
/// у исполнения нет канала к ядру, а запись в реестр без отправки ядру скрыла бы смену от лёгкого чтения следующего
/// блока (per_block_aero::put отправляет пул ядру только при отличии от реестра) — реестр и ядро обновит оно. Принимает: котировку, пул шага, адрес пула. Ошибки: NoRoute — нет tickSpacing / комиссии
/// записи, комиссия выросла; как aero_cl::fee_latest.
async fn aero_fresh(q: &Quoting, e: &Entry, pool: Address) -> GatewayResult<()> {
    let (ctx, f) = (&q.ctx, e.family);
    let (Some(spacing), Some(was)) = (e.pool.tick_spacing, e.pool.fee_pips) else {
        let why = format!(
            "{}: у пула Aero Slipstream V3 нет tickSpacing или комиссии",
            f.id
        );
        return Err(ctx.err(ErrorCode::NoRoute, why));
    };
    let now = if f.variant == Variant::SlipstreamDynamicFee {
        super::super::pools::fee_fresh::read(ctx, pool, "latest").await?
    } else {
        aero_cl::fee_latest(ctx, f, pool, spacing).await?
    };
    if now <= was {
        return Ok(());
    }
    Err(ctx.err(
        ErrorCode::NoRoute,
        format!(
            "{}: комиссия пула {now} пипс выше котировки {was} — своп не отправлен",
            f.id
        ),
    ))
}

/// Свежие ограничения фабрики DEX `id` — в реестр, если у пулов шагов этой DEX в памяти другие: всем пулам Solidly
/// этой DEX (фабрика одна на DEX). Совпали — реестр не трогается (проход реестра только при изменении). Ошибок нет.
fn remember(reg: &Registry, entries: &[&Entry], id: &str, g: SolidlyGate) {
    let stale = entries.iter().any(|e| {
        e.family.id == id && matches!(&e.pool.state, PoolState::Solidly { gate, .. } if *gate != g)
    });
    if !stale {
        return;
    }
    for r in reg.refs() {
        reg.update(&r, |x| {
            if x.family.id == id
                && let PoolState::Solidly { gate, .. } = &mut x.pool.state
            {
                *gate = g;
            }
        });
    }
}
