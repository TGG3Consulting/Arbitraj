//! Файл: venues/bsc_dex/pools/load.rs
//! Что делает: чтение состояния любого пула на заданном блоке (по его виду) и сборка записи реестра
//! из найденного пула; перечитывание записи (после реорга, обрыва или расхождения карты с событием).
//! Кто вызывает: pools/listen.rs.
//! От чего зависит: load_v2v3.rs, load_cl.rs, load_bin.rs, registry.rs, discover.rs, tokens.rs.
//! Настройки: через вызываемые файлы (dex.tick_words, dex.bin_depth_usd, dex.tick_lens, dex.state_view).

use alloy_primitives::Address;
use chrono::Utc;

use crate::core::types::{Pool, PoolKind, PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::tokens::Tokens;
use super::bins::BinMap;
use super::discover::Found;
use super::families::Family;
use super::families::Proto;
use super::hooks::DYNAMIC_FEE_FLAG;
use super::registry::Entry;
use super::ticks::TickMap;
use super::{load_bin, load_cl, load_dodo, load_elastic, load_v2v3};

/// Прочитанное состояние.
pub struct Loaded {
    pub state: PoolState,
    pub ticks: Option<TickMap>,
    pub bins: Option<BinMap>,
}

/// Чтение состояния пула на блоке.
/// Принимает: вид, ссылку, tickSpacing, binStep, USDT и token1 (кто из них Y у Bin), токены (decimals USDT), блок.
/// Ошибки: не задана нужная настройка, узел, ответ не по ABI, нет tickSpacing/binStep у пула.
#[allow(clippy::too_many_arguments)]
pub async fn state(
    ctx: &Ctx,
    tokens: &Tokens,
    f: &Family,
    reference: &PoolRef,
    spacing: Option<i32>,
    step: Option<u32>,
    usdt: Address,
    token1: Address,
    block: u64,
) -> GatewayResult<Loaded> {
    let tag = block_tag(block);
    let missing = |what: &str| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{}: у пула нет {what}", f.title),
        )
    };
    // DODO PMM (решение 86b): вид ядра Pmm, пул — отдельный контракт, чтение своё (load_dodo.rs): кривая
    // getPMMStateForCall + комиссии getUserFeeRate + ориентация base/quote. Карт тиков/бинов нет.
    if f.proto.is_dodo() {
        if let PoolRef::Contract(a) = reference {
            return Ok(Loaded {
                state: load_dodo::load(ctx, *a, &tag).await?,
                ticks: None,
                bins: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул DODO не того вида", f.title),
        ));
    }
    // KyberSwap Elastic (решение 81): вид ядра Cl, но пул — отдельный контракт, чтение своё (load_elastic.rs).
    if f.proto == Proto::KyberElastic {
        if let PoolRef::Contract(a) = reference {
            let sp = spacing.ok_or_else(|| missing("tickDistance"))?;
            let (state, t) = load_elastic::load(ctx, f, *a, sp, &tag).await?;
            return Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Elastic не того вида", f.title),
        ));
    }
    match (f.kind, reference) {
        (PoolKind::V2, PoolRef::Contract(a)) => Ok(Loaded {
            state: load_v2v3::v2(ctx, *a, &tag).await?,
            ticks: None,
            bins: None,
        }),
        (PoolKind::V3, PoolRef::Contract(a)) => {
            let sp = spacing.ok_or_else(|| missing("tickSpacing"))?;
            let (state, t) = load_v2v3::v3(ctx, f, *a, sp, &tag).await?;
            Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
            })
        }
        (PoolKind::Cl, PoolRef::Singleton { manager, id }) => {
            let sp = spacing.ok_or_else(|| missing("tickSpacing"))?;
            let (state, t) = load_cl::load(ctx, f, *manager, *id, sp, &tag).await?;
            Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
            })
        }
        (PoolKind::Bin, PoolRef::Singleton { manager, id }) => {
            let st = step.ok_or_else(|| missing("binStep"))?;
            let depth = load_bin::Depth {
                usdt_is_y: usdt == token1,
                usdt_decimals: tokens.decimals(ctx, usdt).await?,
            };
            let (state, b) = load_bin::load(ctx, *manager, *id, st, &depth, &tag).await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: Some(b),
            })
        }
        _ => Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул не того вида", f.title),
        )),
    }
}

/// Запись реестра из найденного пула: читает состояние на блоке.
/// Ошибки: как у state.
pub async fn entry(
    ctx: &Ctx,
    tokens: &Tokens,
    p: &Found,
    ticker: &str,
    usdt: Address,
    block: u64,
) -> GatewayResult<Entry> {
    let l = state(
        ctx,
        tokens,
        p.family,
        &p.reference,
        p.tick_spacing,
        p.bin_step,
        usdt,
        p.token1,
        block,
    )
    .await?;
    Ok(built(ctx, p, ticker, usdt, block, l))
}

/// Запись реестра из найденного пула и уже прочитанного состояния (и пачечное чтение, load_batch.rs). Ошибок нет.
pub fn built(ctx: &Ctx, p: &Found, ticker: &str, usdt: Address, block: u64, l: Loaded) -> Entry {
    let fee_pips = p.fee_pips.filter(|f| *f != DYNAMIC_FEE_FLAG);
    let pool = Pool {
        venue: ctx.venue(),
        family: p.family.id.to_string(),
        kind: p.family.kind,
        reference: p.reference.clone(),
        token0: p.token0,
        token1: p.token1,
        fee_pips,
        tick_spacing: p.tick_spacing,
        bin_step: p.bin_step,
        hooks: p.hooks.clone(),
        state: l.state,
        block,
        received_at: Utc::now(),
    };
    Entry {
        family: p.family,
        pool,
        ticker: ticker.to_string(),
        coin: p.coin,
        usdt,
        key: p.key.clone(),
        ticks: l.ticks,
        bins: l.bins,
        synced_block: block,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// Перечитывает состояние записи на блоке. Принимает: запись. Возвращает: обновлённую запись (valid = true).
/// Ошибки: как у state.
pub async fn refresh(ctx: &Ctx, tokens: &Tokens, e: &Entry, block: u64) -> GatewayResult<Entry> {
    let p = &e.pool;
    let l = state(
        ctx,
        tokens,
        e.family,
        &p.reference,
        p.tick_spacing,
        p.bin_step,
        e.usdt,
        p.token1,
        block,
    )
    .await?;
    Ok(renewed(e, block, l))
}

/// Запись с новым состоянием на блоке (valid = true). Ошибок нет.
pub fn renewed(e: &Entry, block: u64, l: Loaded) -> Entry {
    let mut out = e.clone();
    out.pool.state = l.state;
    out.pool.block = block;
    out.pool.received_at = Utc::now();
    out.ticks = l.ticks;
    out.bins = l.bins;
    out.synced_block = block;
    out.synced_log = None;
    out.valid = true;
    out
}
