//! Файл: venues/bsc_dex/exec/precheck_quoter.rs
//! Что делает: сверка с квотером своей DEX каждого хопа маршрута, посчитанного портом чужой математики, прямо перед
//! сделкой через исполнитель (решение 73; решение 122 F — Maverick V2: расчёт — порт команды Maverick, Solidity-исходник
//! закрыт, поэтому каждый обмен через такой пул сверяется с MaverickV2Quoter.calculateSwap на своём узле на сумме
//! хопа). Расхождение больше dex.quoter_max_diff_pct, неполный расчёт или откат квотера — сделки нет (exec/precheck.rs
//! quoter_now); пул из котировок исключает цикл сверки (check/quoter_cycle.rs).
//! Решение 177: у продажи выхода (TradeRequest.exit_position) расхождение такого хопа — продажа по квотеру (hops_exit,
//! exec/exit_quote.rs). Решение 261: у остальных сверка односторонняя — квотер меньше расчёта больше допуска — отказ;
//! больше — сделка идёт, строка «в нашу сторону» в хронологию (exec/quoter_favour.rs; сухая сделка — без строки).
//! Кто вызывает: exec/executor/trade.rs (prepare — доли перед отправкой, hops_exit), exec/dry_build.rs (hops).
//! От чего зависит: exec/precheck.rs (quoter_now), exec/exit_quote.rs, exec/quoter_favour.rs, quote/path.rs,
//! pools/families.rs.
//! Настройки: через quoter_now — dex.quoter [DEX], dex.quoter_max_diff_pct [bsc_dex].

use rust_decimal::Decimal;

use crate::core::types::TradeRequest;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::families::Proto;
use super::super::quote::path::Path;
use super::{exit_quote, precheck, quoter_favour};

/// Виды пулов, у которых расчёт — порт без официального исходника: их хоп сверяется квотером перед каждой сделкой.
/// Ошибок нет.
pub fn must_quote(p: Proto) -> bool {
    p == Proto::Maverick
}

/// Сверка хопов таких пулов во всех долях. Принимает: окружение, доли (путь, количество). Ошибки: как у quoter_now
/// (NoRoute, QuoteMismatch, NotConfigured, узел).
pub async fn hops(ctx: &Ctx, paths: &[(Path, Decimal)]) -> GatewayResult<()> {
    for h in paths.iter().flat_map(|(p, _)| p.hops.iter()) {
        if must_quote(h.entry.family.proto) {
            precheck::quoter_now(ctx, &h.entry, h.token_in, &h.swapped, h.usable.hook_pct).await?;
        }
    }
    Ok(())
}

/// Как hops, но по запросу сделки (решение 177): продажа выхода с расхождением — доля пересчитана по квотеру
/// (exec/exit_quote.rs rescale). Принимает: окружение, запрос, доли. Возвращает: множитель каждой доли (1 — без
/// поправки; им же умножается свежий минимум верификации доли) и строку «— продажа по квотеру» / «в нашу сторону»
/// (решение 261; None — поправок нет). Ошибки: как у hops; BadResponse (множитель).
pub async fn hops_exit(
    ctx: &Ctx,
    req: &TradeRequest,
    paths: &mut [(Path, Decimal)],
) -> GatewayResult<(Vec<Decimal>, Option<String>)> {
    let exit = exit_quote::exit_of(req);
    let (mut ratios, mut all, mut favour) =
        (Vec::with_capacity(paths.len()), Vec::new(), Vec::new());
    for (p, _) in paths.iter_mut() {
        let got = exit_quote::check(ctx, p, exit, |h| must_quote(h.entry.family.proto)).await?;
        let (fixes, fav) = quoter_favour::split(got);
        favour.extend(fav);
        ratios.push(if fixes.is_empty() {
            Decimal::ONE
        } else {
            exit_quote::rescale(ctx, p, req.side, &fixes)?
        });
        all.extend(fixes);
    }
    let fav = quoter_favour::report(ctx, req, &favour).await;
    Ok((
        ratios,
        quoter_favour::join(exit_quote::report(ctx, req, &all).await, fav),
    ))
}
