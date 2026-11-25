//! Compute a numeric alternate while ordinary precheck reads balances and network rules.
//! This work cannot generate an order or provide the price used to admit one.
use super::{Core, DirCtx, Direction, Stamp, alternative, saved, stamp};
use rust_decimal::Decimal;
use std::{cell::Cell, future::Future};

#[derive(Clone, Copy)]
struct Hint {
    anchor: Decimal,
    qty: Decimal,
    mark: Stamp,
    budget: Decimal,
    step: Decimal,
    floor: (Decimal, Decimal),
    keep: Decimal,
}
tokio::task_local! {static READY:Cell<Option<Hint>>;}

pub(in crate::core) async fn during<F: Future>(
    core: &Core,
    d: &Direction,
    qty: Option<Decimal>,
    normal: F,
) -> F::Output {
    let Some((qty, samples)) = qty.and_then(|q| saved(core, d, q).map(|s| (q, s))) else {
        return normal.await;
    };
    let Some(floor) = samples.floor else {
        return normal.await;
    };
    let warm = async {
        let slot = core.inner.hint_slot.clone().try_acquire_owned().ok()?;
        let cx = DirCtx::build(core, d).ok()?;
        let (q, mark, _slot) =
            alternative(core, &cx, qty, samples.budget, samples.step, floor, slot).await?;
        Some(Hint {
            anchor: qty,
            qty: q,
            mark,
            budget: samples.budget,
            step: samples.step,
            floor,
            keep: cx.keep?,
        })
    };
    READY
        .scope(Cell::new(None), async {
            tokio::pin!(normal, warm);
            tokio::select! {biased;
                output=&mut normal=>output,
                hint=&mut warm=>{
                    READY.with(|cell|cell.set(hint));
                    normal.await
                }
            }
        })
        .await
}
pub(in crate::core) fn ready(
    core: &Core,
    cx: &DirCtx,
    anchor: Decimal,
    budget: Decimal,
    step: Decimal,
    floor: (Decimal, Decimal),
) -> Option<(Decimal, Stamp)> {
    let h = READY.try_with(Cell::get).ok()??;
    (h.anchor == anchor
        && h.budget == budget
        && h.step == step
        && h.floor == floor
        && Some(h.keep) == cx.keep
        && stamp(core, cx) == Some(h.mark))
    .then_some((h.qty, h.mark))
}
