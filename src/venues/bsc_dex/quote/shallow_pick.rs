//! Файл: venues/bsc_dex/quote/shallow_pick.rs
//! Что делает: решение 390 (ревью t178, Б2) — сколько мелких пулов пары «монета — встречный токен» допускается к пробе
//! налогов, сверке мелкой стороны с квотером, полной подписке (ws) и графу: dex.shallow_pools_per_pair. Мелкий — пул, не
//! глубокий в оба направления (решение 73, dex.min_pool_liquidity_usd в полосе dex.depth_band_pct). Глубокие стороны и
//! пулы — как раньше, предел их не касается.
//! Отбор по рангу (данные пула, без чисел в коде): сначала наибольшая сумма пробы check.probe_usdt (или её доля 1/2^k),
//! которую пропускает хоть одна сторона пула (quote/side_fit.rs side_usd), затем бóльшая глубина стороны в полосе, затем
//! адрес пула (чтобы равные ранги не делили место). Пул без проходящей стороны — не кандидат.
//! Не задана или 0 — мелкие пулы не допускаются (как до решения 390). Рост нагрузки от решения 390 — только через эту
//! настройку.
//! Кто вызывает: check/taxes_util.rs (probe_pools), check/quoter_one.rs, check/quoter_side.rs, pools/depth.rs,
//! pools/graph.rs, pools/graph_eval.rs, quote/select.rs (limit).
//! От чего зависит: quote/side_fit.rs, quote/select.rs (deep_side), pools/depth_book.rs (глубина, label), registry.rs,
//! tokens.rs.
//! Настройки: dex.shallow_pools_per_pair [bsc_dex]; через side_fit — check.probe_usdt, dex.quoter_max_diff_pct.

use rust_decimal::Decimal;

use crate::core::types::Side;

use super::super::ctx::Ctx;
use super::super::pools::depth_book;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::{select, side_fit};

/// Имя настройки предела.
pub const NAME: &str = "dex.shallow_pools_per_pair";

/// Предел мелких пулов на пару. Не задана, неверна — 0 (мелкие не допускаются). Ошибок нет.
pub fn limit(ctx: &Ctx) -> usize {
    ctx.int(NAME)
        .ok()
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0)
}

/// Ранг пула: (наибольшая проходящая сумма пробы, USDT; бóльшая глубина стороны в полосе, USDT). None — ни одна сторона
/// не проходит. Ошибок нет (глубину не посчитать — 0).
pub fn rank(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    e: &Entry,
) -> Option<(Decimal, Decimal)> {
    let fit = [Side::Buy, Side::Sell]
        .into_iter()
        .filter_map(|s| side_fit::side_usd(ctx, reg, tokens, usdt_dec, e, s).map(|x| x.1))
        .max()?;
    let depth = reg
        .depth()
        .depth(ctx, reg, tokens, e, usdt_dec)
        .map(|d| d.band.buy.max(d.band.sell))
        .unwrap_or(Decimal::ZERO);
    Some((fit, depth))
}

/// Глубок ли пул в оба направления (тогда он не мелкий и предел его не касается). Ошибок нет.
fn deep_both(ctx: &Ctx, reg: &Registry, tokens: &Tokens, usdt_dec: u8, e: &Entry) -> bool {
    matches!(
        select::deep_side(ctx, reg, tokens, e, usdt_dec, select::Need::Both),
        Ok(None)
    )
}

/// Допущен ли мелкий пул `e` (см. шапку): предел больше 0, у пула есть проходящая сторона, и пулов той же пары с рангом
/// выше — меньше предела. Принимает: окружение, реестр, токены, знаки USDT, пул. Ошибок нет.
pub fn admitted(ctx: &Ctx, reg: &Registry, tokens: &Tokens, usdt_dec: u8, e: &Entry) -> bool {
    let n = limit(ctx);
    if n == 0 {
        return false;
    }
    let Some(mine) = rank(ctx, reg, tokens, usdt_dec, e) else {
        return false;
    };
    let me = depth_book::label(&e.pool.reference);
    let mut better = 0usize;
    for x in reg.by_coin(&e.coin) {
        if !x.valid || x.usdt != e.usdt || x.pool.reference == e.pool.reference {
            continue;
        }
        if deep_both(ctx, reg, tokens, usdt_dec, &x) {
            continue;
        }
        let Some(r) = rank(ctx, reg, tokens, usdt_dec, &x) else {
            continue;
        };
        if r > mine || (r == mine && depth_book::label(&x.pool.reference) < me) {
            better += 1;
            if better >= n {
                return false;
            }
        }
    }
    true
}
