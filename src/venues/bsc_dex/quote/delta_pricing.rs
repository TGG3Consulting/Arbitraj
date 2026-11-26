//! Отдельный нативный Router02 конкурирует с прочими полными способами исполнения.
use super::super::{
    pools::{deltaswap, family_variant::Variant, registry::Entry},
    wallet::signer,
};
use super::{
    Quoting,
    routing::Input,
    v2_execution::{self, Execution, OptionQuote},
    white_state,
};
use crate::core::types::RouteMode;

pub(super) fn is_delta(e: &Entry) -> bool {
    e.family.variant == Variant::DeltaSwap
}

pub(super) async fn candidates(q: &Quoting, i: &Input) -> Vec<OptionQuote> {
    let Ok(payer) = signer::address(&q.ctx) else {
        return Vec::new();
    };
    let head = if q.exec_quote {
        let Ok(head) = white_state::head(&q.ctx, "latest").await else {
            return Vec::new();
        };
        Some(head)
    } else {
        None
    };
    let mut options = Vec::new();
    for mut e in i.entries.iter().filter(|e| is_delta(e)).cloned() {
        if let Some(h) = head
            && deltaswap::refresh(&q.ctx, &mut e, h).await.is_err()
        {
            continue;
        }
        let Ok(target) = deltaswap::router(&q.ctx, &e) else {
            continue;
        };
        let execution = Execution {
            mode: RouteMode::Direct,
            target,
            step: None,
        };
        if let Ok(option) = v2_execution::option(q, i, e, payer, execution, head).await {
            options.push(option);
        }
    }
    options
}
