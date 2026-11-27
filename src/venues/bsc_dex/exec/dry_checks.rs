//! Файл: venues/bsc_dex/exec/dry_checks.rs
//! Что делает: сверки с квотером сухой сборки (exec/dry_build_once.rs) — ТЕ ЖЕ, что у настоящей сделки той же формы:
//! - прямая форма (direct) — как exec/trade_direct_prep.rs: хопы квотером пула; продажа выхода (TradeRequest.exit_position,
//!   решение 177) с расхождением — не отказ, а путь по квотеру (exec/exit_quote.rs rescale; свежий минимум графовой
//!   верификации — тем же множителем); остальное — односторонне (решение 261). Решение 382 (выравнивание 234 с 177):
//!   симуляция продажи до покупки (exec/presell.rs, exit_position = true) больше не строже самой продажи — расхождение
//!   ведёт к продаже по квотеру и в симуляции, а выгодность решает ядро по выходу симуляции (core/precheck_dex_sell.rs:
//!   «прибыль по квотеру ниже порога»);
//! - умный / граф «один UR» (smart) — MixedQuoter (quote/mixed_quoter.rs), как exec/smart_legs.rs;
//! - хопы Maverick (maverick) — exec/precheck_quoter.rs hops.
//!
//! Квотер — на блоке перечитанных пулов (Quoting.pinned, решение 382), иначе «latest».
//! Кто вызывает: exec/dry_build_once.rs.
//! От чего зависит: exec/exit_quote.rs, exec/quoter_favour.rs, exec/quoter_refetch.rs, exec/quoter_refetch_check.rs,
//! exec/precheck_quoter.rs, quote/mixed_quoter.rs (Leg), quote/path.rs.
//! Настройки: через вызываемые — dex.quoter [вид пула], smart.mixed_quoter, dex.quoter_max_diff_pct [bsc_dex].

use rust_decimal::Decimal;

use crate::core::types::TradeRequest;
use crate::gateway::errors::GatewayResult;

use super::super::quote::Quoting;
use super::super::quote::mixed_quoter::Leg;
use super::super::quote::path::{Hop, Path};
use super::{exit_quote, precheck_quoter, quoter_favour, quoter_refetch, quoter_refetch_check};

/// Прямая форма (см. шапку). Принимает: окружение, запрос, доли (пути — меняются при продаже по квотеру), свежие
/// минимумы графовой верификации (тем же множителем). Возвращает: строку «продажа по квотеру» (None — по расчёту).
/// Ошибки: как exit_quote::check (NoRoute, QuoteMismatch, NotConfigured, узел); BadResponse (множитель).
pub async fn direct(
    q: &Quoting,
    req: &TradeRequest,
    paths: &mut [(Path, Decimal)],
    mins: &mut Option<Vec<Decimal>>,
) -> GatewayResult<Option<String>> {
    let ctx = &q.ctx;
    let exit = exit_quote::exit_of(req);
    let at = quoter_refetch::block(q);
    let mut all = Vec::new();
    for (p, _) in paths.iter_mut() {
        let pick = |h: &Hop| !super::carbon_direct::enabled(&h.entry);
        let got = match at {
            Some(b) => quoter_refetch_check::check_on(ctx, p, (exit, pick), b).await?,
            None => exit_quote::check(ctx, p, exit, pick).await?,
        };
        // Строки «в нашу сторону» сухой сборке не нужны (сделки нет) — как раньше.
        let (fixes, _) = quoter_favour::split(got);
        if fixes.is_empty() {
            continue;
        }
        let r = exit_quote::rescale(ctx, p, req.side, &fixes)?;
        if let Some(m) = mins.as_mut() {
            *m = exit_quote::scale_mins(m.as_slice(), r);
        }
        all.extend(fixes);
    }
    if all.is_empty() {
        return Ok(None);
    }
    let line = all
        .iter()
        .map(|(_, f)| f.text())
        .collect::<Vec<_>>()
        .join("; ");
    Ok(Some(format!("{line} (решение 177)")))
}

/// Умный маршрут: MixedQuoter по долям. Принимает: окружение, доли. Ошибки: как quote/mixed_quoter.rs check.
pub async fn smart(q: &Quoting, paths: &[(Path, Decimal)]) -> GatewayResult<()> {
    let legs: Vec<Leg<'_>> = paths.iter().map(|(p, _)| Leg { path: p }).collect();
    quoter_refetch_check::mixed(&q.ctx, &legs, quoter_refetch::block(q))
        .await
        .map(|_| ())
}

/// Хопы Maverick (решение 122 F). Принимает: окружение, доли. Ошибки: как exec/precheck_quoter.rs hops.
pub async fn maverick(q: &Quoting, paths: &[(Path, Decimal)]) -> GatewayResult<()> {
    precheck_quoter::hops(&q.ctx, paths, quoter_refetch::block(q)).await
}
