//! Файл: venues/bsc_dex/pools/load_batch_ticks.rs
//! Что делает: ступень 3 пачечного чтения (load_batch.rs) — данные установленных тиков пулов CL всех ждущих пулов
//! вместе: Infinity CL — getPoolTickInfo у менеджера, Uniswap V4 — StateView getTickLiquidity; итог — карта тиков.
//! Те же функции и правила, что у чтения по одному (load_cl.rs).
//! Кто вызывает: pools/load_batch.rs.
//! От чего зависит: pools/abi_v4.rs, node/call.rs (decode), load_batch_cl.rs (target), load_batch_v2v3.rs (run),
//! load_cl.rs (i24).
//! Настройки: нет своих (dex.state_view — через target).
//!
//! getPoolTickInfo — ICLPoolManager.sol#L89-L108; getTickLiquidity — StateView.sol#L19-L77 (research/flow/bsc_v4.md,
//! раздел 4; как в load_cl.rs).

use std::collections::BTreeMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolState;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_v4::{IInfinityCl, IStateView};
use super::families::Proto;
use super::load::Loaded;
use super::load_batch::{Miss, miss};
use super::load_batch::{Out, Stage, Want};
use super::load_batch_cl::target;
use super::load_batch_v2v3::run;
use super::load_cl;
use super::ticks::{TickData, TickMap};

/// Вызовы данных тиков. Ошибки: как у target; тик вне int24.
pub fn tick_calls(ctx: &Ctx, w: &Want, ticks: &[i32]) -> Result<Vec<(Address, Bytes)>, Miss> {
    let (inf, to, id) = target(ctx, w)?;
    ticks
        .iter()
        .map(|t| {
            let tick = load_cl::i24(ctx, *t)?;
            let d = if inf {
                IInfinityCl::getPoolTickInfoCall { id, tick }.abi_encode()
            } else {
                IStateView::getTickLiquidityCall { poolId: id, tick }.abi_encode()
            };
            Ok((to, Bytes::from(d)))
        })
        .collect()
}

/// Карта тиков из ответов. Ошибки: причина (ответ не по ABI).
pub fn tick_data(w: &Want, ticks: &[i32], ans: &[Bytes]) -> Result<BTreeMap<i32, TickData>, Miss> {
    let m = |e: Miss| e;
    ticks
        .iter()
        .zip(ans)
        .map(|(t, b)| {
            let (gross, net) = if w.family.proto == Proto::InfinityCl {
                let i = call::decode::<IInfinityCl::getPoolTickInfoCall>(b).map_err(m)?;
                (i.liquidityGross, i.liquidityNet)
            } else {
                let i = call::decode::<IStateView::getTickLiquidityCall>(b).map_err(m)?;
                (i.liquidityGross, i.liquidityNet)
            };
            Ok((*t, TickData { gross, net }))
        })
        .collect()
}

/// Пулы CL, ждущие ступени 3: (состояние, установленные тики, tickSpacing, прочитанные слова).
pub type Waiting = Vec<Option<(PoolState, Vec<i32>, i32, (i16, i16))>>;

/// Ступень 3: данные установленных тиков CL всех ждущих пулов вместе — итог в `out`. Ошибок нет: причина — у пула.
pub async fn finish(
    ctx: &Ctx,
    wants: &[Want<'_>],
    mut wait: Waiting,
    out: &mut Out,
    tag: &str,
    size: usize,
) {
    let third: Stage = wants
        .iter()
        .enumerate()
        .map(|(i, w)| match &wait[i] {
            Some((_, t, _, _)) => tick_calls(ctx, w, t).unwrap_or_else(|why| {
                wait[i] = None;
                out[i] = Some(Err(why));
                Vec::new()
            }),
            None => Vec::new(),
        })
        .collect();
    let infos = run(ctx, &third, tag, size).await;
    for (i, w) in wants.iter().enumerate() {
        let Some((state, t, spacing, words)) = wait[i].take() else {
            continue;
        };
        let ticks = infos[i]
            .as_ref()
            .map_err(|t| miss(t.clone()))
            .and_then(|b| tick_data(w, &t, b));
        out[i] = Some(ticks.map(|ticks| Loaded {
            state,
            ticks: Some(TickMap {
                spacing,
                ticks,
                words,
            }),
            bins: None,
        }));
    }
}
