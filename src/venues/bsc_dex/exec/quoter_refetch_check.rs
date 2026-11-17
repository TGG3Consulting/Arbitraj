//! Файл: venues/bsc_dex/exec/quoter_refetch_check.rs
//! Что делает: решение 382 — сверки с квотером на блоке перечитанных пулов (Quoting.pinned, exec/quoter_refetch.rs):
//! хопы пути квотером пула (check_on — как exec/exit_quote.rs check_with), умный маршрут MixedQuoter (mixed —
//! quote/mixed_quoter.rs check_at), хопы Maverick исполнителя и сухой сборки (через exec/precheck_quoter.rs с блоком).
//! Без перечитанных пулов — «latest», как раньше.
//! Кто вызывает: exec/trade_direct_prep.rs, exec/smart_legs.rs, exec/dry_checks.rs, exec/precheck_quoter.rs.
//! От чего зависит: exec/precheck.rs (quoter_hop_at), exec/exit_quote.rs, quote/mixed_quoter.rs, node/call.rs.
//! Настройки: через вызываемые — dex.quoter [вид пула], smart.mixed_quoter, dex.quoter_max_diff_pct [bsc_dex].

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::quote::mixed_quoter::{self, Leg};
use super::super::quote::path::{Hop, Path};
use super::exit_quote::ExitFix;
use super::precheck;

/// Тег блока для eth_call: номер перечитанных пулов (hex) или «latest». Ошибок нет.
pub fn tag(block: Option<u64>) -> String {
    block.map_or_else(|| "latest".to_string(), call::block_tag)
}

/// Сверка с MixedQuoter (решение 261, односторонняя) на блоке перечитанных пулов или «latest». Принимает: окружение,
/// доли, блок. Возвращает и ошибки: как quote/mixed_quoter.rs check.
pub async fn mixed(ctx: &Ctx, legs: &[Leg<'_>], block: Option<u64>) -> GatewayResult<Vec<String>> {
    mixed_quoter::check_at(ctx, legs, &tag(block)).await
}

/// Сверка хопов пути с квотером на блоке `block` (пулы пути перечитаны на нём) — как exec/exit_quote.rs check_with,
/// но без известного выхода пробы: квотер каждого хопа вызывается на этом блоке (precheck::quoter_hop_at). Принимает:
/// окружение, путь, (продажа ли выхода, какие хопы сверять), блок. Возвращает: расхождения (номер хопа, цифры).
/// Ошибки: как exit_quote::check_with; StaleData — состояния блока у узла нет.
pub async fn check_on(
    ctx: &Ctx,
    path: &Path,
    (exit, pick): (bool, impl Fn(&Hop) -> bool),
    block: u64,
) -> GatewayResult<Vec<(usize, ExitFix)>> {
    let mut out = Vec::new();
    for (i, h) in path.hops.iter().enumerate().filter(|(_, h)| pick(h)) {
        let fix = precheck::quoter_hop_at(
            ctx,
            &h.entry,
            h.token_in,
            &h.swapped,
            (h.usable.hook_pct, exit),
            block,
        )
        .await?;
        if let Some(f) = fix {
            out.push((i, f));
        }
    }
    Ok(out)
}
