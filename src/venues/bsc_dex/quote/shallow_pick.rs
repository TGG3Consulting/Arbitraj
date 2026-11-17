//! Файл: venues/bsc_dex/quote/shallow_pick.rs
//! Что делает: решение 390 (ревью t178, Б2) — сколько мелких пулов пары «монета — встречный токен» допускается к пробе
//! налогов, сверке мелкой стороны с квотером, полной подписке (ws) и графу: dex.shallow_pools_per_pair. Мелкий — пул, не
//! глубокий в оба направления (решение 73, dex.min_pool_liquidity_usd в полосе dex.depth_band_pct). Глубокие стороны и
//! пулы — как раньше, предел их не касается.
//! Отбор по рангу (данные пула, без чисел в коде; повторное ревью, Н-B1 — устойчиво к обменам):
//! - уровень — наибольшая доля суммы пробы check.probe_usdt из ряда 1, 1/2, 1/4, …, которую целиком пропускает хоть одна
//!   сторона пула (quote/side_fit.rs side_usd); уровень меняется, только когда пул перестаёт пропускать долю ряда
//!   (у V2 — всегда полная сумма), глубина в полосе в ранг НЕ входит (она меняется каждым обменом);
//! - при равном уровне — уже отобранный пул остаётся (гистерезис на состоянии, quote/shallow_memo.rs), затем адрес пула.
//!
//! Пул без проходящей стороны — не кандидат. Отбор пары считается один раз на версию реестра и настройки
//! (quote/shallow_memo.rs, Н-B2). Не задана или 0 — мелкие пулы не допускаются (как до решения 390).
//! Кто вызывает: check/taxes_util.rs (probe_pools), check/quoter_one.rs, check/quoter_side.rs, pools/depth.rs,
//! pools/graph.rs, pools/graph_eval.rs, quote/select.rs (limit).
//! От чего зависит: quote/side_fit.rs, quote/select.rs (deep_side), quote/shallow_memo.rs, pools/depth_book.rs (label),
//! registry.rs, tokens.rs.
//! Настройки: dex.shallow_pools_per_pair [bsc_dex]; через side_fit — check.probe_usdt, dex.quoter_max_diff_pct; ключ
//! памяти — ещё venue.quote_asset, dex.depth_band_pct, dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct.

use std::sync::Arc;

use alloy_primitives::{Address, U256};

use crate::core::types::{PoolRef, Side};

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

/// Уровень пула: наименьшее k, при котором доля суммы пробы 1/2^k целиком проходит хоть в одну сторону (меньше —
/// лучше). None — ни одна сторона не проходит. Ошибок нет.
pub fn level(ctx: &Ctx, reg: &Registry, tokens: &Tokens, usdt_dec: u8, e: &Entry) -> Option<u32> {
    let usdt = tokens.usdt(ctx).ok()?;
    let cap = side_fit::cap(ctx, reg, usdt, usdt_dec, e).ok()?;
    [Side::Buy, Side::Sell]
        .into_iter()
        .filter_map(|s| side_fit::side_usd(ctx, reg, tokens, usdt_dec, e, s).map(|x| x.0))
        .map(|a| {
            let mut k = 0u32;
            while (k as usize) < U256::BITS && (cap >> (k as usize)) > a {
                k += 1;
            }
            k
        })
        .min()
}

/// Кандидат отбора: уровень, был ли в прошлом отборе пары, адрес текстом, ссылка.
pub type Cand = (u32, bool, String, PoolRef);

/// Порядок отбора (чистая логика): уровень по возрастанию, при равном — бывший в отборе первым, затем адрес; первые `n`.
/// Ошибок нет.
pub fn order(mut c: Vec<Cand>, n: usize) -> Vec<PoolRef> {
    c.sort_by_key(|x| (x.0, std::cmp::Reverse(x.1), x.2.clone()));
    c.into_iter().take(n).map(|x| x.3).collect()
}

/// Глубок ли пул в оба направления (тогда он не мелкий и предел его не касается). Ошибок нет.
fn deep_both(ctx: &Ctx, reg: &Registry, tokens: &Tokens, usdt_dec: u8, e: &Entry) -> bool {
    matches!(
        select::deep_side(ctx, reg, tokens, e, usdt_dec, select::Need::Both),
        Ok(None)
    )
}

/// Ключ памяти отбора: версия реестра и настройки, от которых зависит отбор. Ошибок нет.
fn key(ctx: &Ctx, reg: &Registry) -> (u64, Vec<String>) {
    let t = |n: &str| ctx.text(n).unwrap_or_default();
    let d = |n: &str| {
        ctx.dec(n)
            .map(|v| v.normalize().to_string())
            .unwrap_or_default()
    };
    (
        reg.version(),
        vec![
            limit(ctx).to_string(),
            d("check.probe_usdt"),
            d("dex.quoter_max_diff_pct"),
            d("dex.depth_band_pct"),
            d("dex.min_pool_liquidity_usd"),
            d("dex.depth_hysteresis_pct"),
            t("venue.quote_asset"),
        ],
    )
}

/// Отбор пары «монета — встречный токен» (см. шапку): один раз на ключ памяти. Ошибок нет.
pub fn pick(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    pair: (Address, Address),
) -> Arc<Vec<PoolRef>> {
    let n = limit(ctx);
    if n == 0 {
        return Arc::default();
    }
    reg.shallow_memo().get(key(ctx, reg), pair, |prev| {
        let cands = reg
            .by_coin(&pair.0)
            .into_iter()
            .filter(|x| x.valid && x.usdt == pair.1)
            .filter(|x| !deep_both(ctx, reg, tokens, usdt_dec, x))
            .filter_map(|x| {
                let lv = level(ctx, reg, tokens, usdt_dec, &x)?;
                let r = x.pool.reference.clone();
                Some((lv, prev.contains(&r), depth_book::label(&r), r))
            })
            .collect();
        order(cands, n)
    })
}

/// Допущен ли мелкий пул `e`: он в отборе своей пары (pick). Ошибок нет.
pub fn admitted(ctx: &Ctx, reg: &Registry, tokens: &Tokens, usdt_dec: u8, e: &Entry) -> bool {
    limit(ctx) > 0 && pick(ctx, reg, tokens, usdt_dec, (e.coin, e.usdt)).contains(&e.pool.reference)
}
