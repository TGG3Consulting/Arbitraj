//! Файл: venues/bsc_dex/exec/precheck.rs
//! Что делает: проверки прямо перед сделкой (решение 32: «перед каждой сделкой — повторная пробная проверка и
//! „получить не меньше“»): сверка обмена с квотером своей DEX на реальной сумме сделки (прямой режим; умный сверяет доли MixedQuoter —
//! quote/mixed_quoter.rs). Решение 177: у продажи выхода расхождение — не отказ, а продажа по квотеру (quoter_hop).
//! Решение 261 (Тигран 12.11: «если в нашу сторону то ок если наоборот то не делаем»): у остальных сделок сверка
//! односторонняя — выход квотера меньше нашего больше допуска — отказ; больше (на любую величину) — сделка идёт,
//! поправка с favour = true (только строка в хронологию; ожидаемое и минимум — по нашему расчёту).
//! Повторная проба крючков B/C на сумме сделки (решение 32, «335 (лесенка влияния)») — exec/trade_probe.rs и
//! exec/hook_trade.rs. Ревью Н7: у пула с крючком выход квотера на сумме шага уже снят пробой — quoter_hop_with берёт
//! его без второго вызова квотера.
//! Решение 382: quoter_hop_at / quoter_now_at — квотер на блоке перечитанных пулов (exec/quoter_refetch.rs).
//! Кто вызывает: exec/exit_quote.rs, exec/dry_build.rs, exec/precheck_quoter.rs.
//! От чего зависит: node/call.rs (block_tag), quote/quoter.rs, quote/emptiness.rs, quote/quoter_revert.rs, pools/registry.rs.
//! Настройки: через вызываемые — dex.quoter [вид пула], dex.router (V2), dex.quoter_max_diff_pct.
//!
//! Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5; «Сверка:
//! периодически и перед сделкой — квотер» — research/flow/bsc_dex.md, схема адаптера п. 6.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::swap_cl::Swapped;
use super::super::node::call;
use super::super::pools::registry::Entry;
use super::super::quote::quoter::Judged;
use super::super::quote::{emptiness, quoter, quoter_revert};
use super::exit_quote::ExitFix;

/// Сверка обмена сделки с квотером своей DEX. Принимает: пул, токен входа, локальный обмен в пуле (сколько доходит
/// до пула, выход, полон ли расчёт), влияние крючка на эту сторону (%). Пустой пул или неполный расчёт — квотер не
/// вызывается (quote/emptiness.rs). Ошибки: NoRoute (пустой пул, не хватает ликвидности), QuoteMismatch (расхождение —
/// с цифрами), настройки, узел (откат квотера — его текст).
pub async fn quoter_now(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    local: &Swapped,
    hook_pct: Decimal,
) -> GatewayResult<()> {
    quoter_hop(ctx, e, token_in, local, hook_pct, false)
        .await
        .map(|_| ())
}

/// Как quoter_now, но продажа выхода (решение 177, exit = true): расхождение больше допуска с цифрами (Judged::Diff) —
/// не отказ, а Some(ExitFix) — продать по выходу квотера (exec/exit_quote.rs). Пустой пул, неполный расчёт, откат
/// квотера, «сравнить нельзя» — отказ, как у quoter_now. exit = false — ровно quoter_now. Возвращает: None — в допуске;
/// Some(ExitFix с favour = true) — решение 261, расхождение в нашу сторону (exit = false). Ошибки: как у quoter_now.
pub async fn quoter_hop(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    local: &Swapped,
    hook_pct: Decimal,
    exit: bool,
) -> GatewayResult<Option<ExitFix>> {
    quoter_hop_with(ctx, e, token_in, local, (hook_pct, exit), None).await
}

/// Как quoter_hop; `known` — выход квотера на этой сумме, уже снятый пробой этой сделки (ревью Н7, exec/hook_trade.rs
/// quoted): квотер не вызывается второй раз. Принимает: как quoter_hop ((влияние, продажа выхода)), известный выход.
/// Ошибки: как quoter_hop.
pub async fn quoter_hop_with(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    local: &Swapped,
    (hook_pct, exit): (Decimal, bool),
    known: Option<U256>,
) -> GatewayResult<Option<ExitFix>> {
    hop(ctx, e, token_in, local, (hook_pct, exit), known, "latest").await
}

/// Решение 382: как quoter_hop, но квотер — на блоке `block` (пул перечитан на нём, exec/quoter_refetch.rs): расчёт
/// и квотер на одном блоке. Принимает: как quoter_hop ((влияние, продажа выхода)), блок. Ошибки: как quoter_hop;
/// StaleData — состояния блока у узла нет.
pub async fn quoter_hop_at(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    local: &Swapped,
    (hook_pct, exit): (Decimal, bool),
    block: u64,
) -> GatewayResult<Option<ExitFix>> {
    let tag = call::block_tag(block);
    hop(ctx, e, token_in, local, (hook_pct, exit), None, &tag).await
}

/// Решение 382: quoter_now на блоке `block` (сухая сделка по перечитанным пулам). Ошибки: как quoter_hop_at.
pub async fn quoter_now_at(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    local: &Swapped,
    hook_pct: Decimal,
    block: u64,
) -> GatewayResult<()> {
    quoter_hop_at(ctx, e, token_in, local, (hook_pct, false), block)
        .await
        .map(|_| ())
}

/// Сверка хопа: известный выход квотера (`known`) или вызов квотера на блоке `tag` («latest» или номер hex).
/// Принимает и ошибки: как quoter_hop_with.
async fn hop(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    local: &Swapped,
    (hook_pct, exit): (Decimal, bool),
    known: Option<U256>,
    tag: &str,
) -> GatewayResult<Option<ExitFix>> {
    let short = if emptiness::empty(e) {
        Some(emptiness::EMPTY.to_string())
    } else if !local.complete {
        Some(emptiness::short(emptiness::SHORT_LOCAL))
    } else {
        None
    };
    let no_route = |why: String| {
        let msg = format!("{} {}: перед сделкой {why}", e.family.title, e.ticker);
        ctx.err(ErrorCode::NoRoute, msg)
    };
    if let Some(why) = short {
        return Err(no_route(why));
    }
    // Настройки — до обращения к узлу (ревью #65).
    ctx.dec("dex.quoter_max_diff_pct")?;
    let asked = match known {
        Some(r) => Ok(r),
        None => quoter::out_at(ctx, e, token_in, local.amount_in, tag).await,
    };
    let remote = match asked {
        Ok(r) => r,
        Err(err) => {
            return Err(match quoter_revert::liquidity_short(&err) {
                Some(why) => no_route(emptiness::short(why)),
                None => err,
            });
        }
    };
    verdict(
        ctx,
        e,
        quoter::judge(ctx, local.amount_out, remote, hook_pct)?,
        exit,
    )
}

/// Решение по итогу сверки (без узла — проверяется тестами на цифрах живого случая): в допуске — None; расхождение
/// у продажи выхода — Some(ExitFix) (решение 177, по модулю, как было); у остальных расхождение в нашу сторону
/// (квотер больше расчёта) — Some(ExitFix, favour = true) (решение 261); иначе — отказ QuoteMismatch с тем же текстом,
/// что до решения 177. Принимает: окружение, пул, итог сверки, продажа ли выхода. Ошибки: QuoteMismatch.
pub fn verdict(ctx: &Ctx, e: &Entry, j: Judged, exit: bool) -> GatewayResult<Option<ExitFix>> {
    let favour = !exit && j.favours_us();
    match j {
        Judged::Within => Ok(None),
        Judged::Diff {
            local, remote, pct, ..
        } if exit || favour => Ok(Some(ExitFix {
            pool: format!("{} {}", e.family.title, e.ticker),
            local,
            remote,
            pct,
            favour,
        })),
        j => Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!(
                "{} {}: перед сделкой {}",
                e.family.title,
                e.ticker,
                j.reason().unwrap_or_default()
            ),
        )),
    }
}
