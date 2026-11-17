//! Файл: venues/bsc_dex/quote/shallow_pick.rs
//! Что делает: решение 393 (ревью t178, Б2) — сколько мелких пулов пары «монета — встречный токен» допускается к пробе
//! налогов, сверке мелкой стороны с квотером, полной подписке (ws) и графу: dex.shallow_pools_per_pair. Мелкий — пул, не
//! глубокий в оба направления (решение 73, dex.min_pool_liquidity_usd в полосе dex.depth_band_pct). Глубокие стороны и
//! пулы — как раньше, предел их не касается.
//! Отбор по рангу (данные пула, без чисел в коде; повторное ревью, Н-B1 — устойчиво к обменам):
//! - уровень — наибольшая доля суммы пробы check.probe_usdt из ряда 1, 1/2, 1/4, …, которую целиком пропускает хоть одна
//!   сторона пула (quote/side_fit.rs side_usd); уровень меняется, только когда пул перестаёт пропускать долю ряда
//!   (у V2 — всегда полная сумма), глубина в полосе в ранг НЕ входит (она меняется каждым обменом);
//! - при равном уровне — уже отобранный пул остаётся (гистерезис на состоянии, quote/shallow_memo.rs), затем адрес пула.
//!
//! Пул без проходящей стороны — не кандидат. Не задана или 0 — мелкие пулы не допускаются (как до решения 393).
//! Решение 403 (t188): отбор пар считает построитель вне пути котировки (quote/shallow_build.rs) и публикует снимком
//! (quote/shallow_memo.rs):
//! - путь котировки (pools/graph.rs, pools/graph_eval.rs, quote/select.rs) — admitted_snap / on_snap: только чтение
//!   снимка, без общего замка, без настроек, без копий;
//! - пути вне котировки (подписка, сверка с квотером, проба налогов, тесты) — admitted / pick: входы пары те же, что у
//!   слота снимка, — готовый отбор, иначе расчёт сейчас (calc) по живому реестру.
//!
//! Кто вызывает: check/taxes_util.rs (probe_pools), check/quoter_one.rs, check/quoter_side.rs, pools/depth.rs (admitted,
//! pick); pools/graph.rs, pools/graph_eval.rs (admitted_snap); quote/select.rs (on_snap); quote/shallow_build.rs (calc).
//! От чего зависит: quote/side_fit.rs, quote/select.rs (deep_side), quote/shallow_memo.rs, pools/depth_book.rs (label),
//! registry.rs, tokens.rs.
//! Настройки: dex.shallow_pools_per_pair [bsc_dex]; через side_fit — check.probe_usdt, dex.quoter_max_diff_pct; через
//! глубину — dex.depth_band_pct, dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct; venue.quote_asset (USDT). Все
//! они — в отпечатке входов через поколение настроек (settings/store.rs generation).

use std::sync::Arc;

use alloy_primitives::{Address, U256};

use crate::core::types::{PoolRef, Side};

use super::super::ctx::Ctx;
use super::super::pools::depth_book;
use super::super::pools::registry::{Entry, Registry};
use super::super::tokens::Tokens;
use super::shallow_memo::Stamp;
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

/// Отпечаток входов отбора пары сейчас (shallow_memo.rs Stamp). Принимает: окружение, реестр, адрес и знаки USDT,
/// пару. Ошибок нет.
pub fn stamp_now(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    usdt_dec: u8,
    pair: (Address, Address),
) -> Stamp {
    Stamp {
        settings: ctx.settings.generation(),
        usdt,
        dec: usdt_dec,
        pools: reg.pair_stamp(pair.0, pair.1),
        rate: if pair.1 == usdt {
            Vec::new()
        } else {
            reg.pair_stamp(pair.1, usdt)
        },
    }
}

/// Отбор пары по её пулам `list` (см. шапку) — чистый расчёт без памяти: допуск (достоверен, встречный — пары, не
/// глубок в оба направления, есть проходящая сторона), ранг, гистерезис по прошлому отбору `prev`, первые `n`.
/// Принимает: окружение, реестр, токены, знаки USDT, пару, предел, прошлый отбор, пулы пары. Ошибок нет.
#[allow(clippy::too_many_arguments)]
pub fn calc(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    pair: (Address, Address),
    n: usize,
    prev: &[PoolRef],
    list: &[Arc<Entry>],
) -> Vec<PoolRef> {
    let mut cands: Vec<Cand> = Vec::new();
    for x in list {
        let e: &Entry = x;
        if !e.valid || e.usdt != pair.1 || deep_both(ctx, reg, tokens, usdt_dec, e) {
            continue;
        }
        let Some(lv) = level(ctx, reg, tokens, usdt_dec, e) else {
            continue;
        };
        let r = e.pool.reference.clone();
        cands.push((lv, prev.contains(&r), depth_book::label(&r), r));
    }
    order(cands, n)
}

/// Отбор пары «монета — встречный токен» сейчас (см. шапку) — для путей вне котировки (подписка pools/depth.rs, сверка
/// с квотером, проба налогов): входы те же, что у слота опубликованного снимка (shallow_memo.rs Stamp), — готовый
/// отбор; иначе расчёт по живому реестру (calc) с прошлым отбором из снимка. Снимок не меняет (его пишет только
/// построитель, quote/shallow_build.rs). Общих замков с копиями нет. Ошибок нет.
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
    let memo = reg.shallow_memo();
    if let Ok(usdt) = tokens.usdt(ctx) {
        let now = stamp_now(ctx, reg, usdt, usdt_dec, pair);
        let ready = memo.with(|s| {
            s.slots
                .get(&pair)
                .filter(|x| s.limit == n && x.stamp == now)
                .map(|x| Arc::clone(&x.sel))
        });
        if let Some(v) = ready {
            return v;
        }
    }
    let prev = memo
        .with(|s| s.slots.get(&pair).map(|x| Arc::clone(&x.sel)))
        .unwrap_or_default();
    let list = reg.pair_arcs(pair.0, pair.1);
    Arc::new(calc(ctx, reg, tokens, usdt_dec, pair, n, &prev, &list))
}

/// Допущен ли мелкий пул `e`: он в отборе своей пары (pick — расчёт сейчас; пути вне котировки). Ошибок нет.
pub fn admitted(ctx: &Ctx, reg: &Registry, tokens: &Tokens, usdt_dec: u8, e: &Entry) -> bool {
    limit(ctx) > 0 && pick(ctx, reg, tokens, usdt_dec, (e.coin, e.usdt)).contains(&e.pool.reference)
}

/// Решение 403: допущен ли мелкий пул `e` по опубликованному снимку (shallow_memo.rs) — путь котировки (сборка графа,
/// оценка пула фоновым графом). Без общего замка, без чтения настроек и без копий. Ошибок нет.
pub fn admitted_snap(reg: &Registry, e: &Entry) -> bool {
    reg.shallow_memo()
        .with(|s| s.admits(&(e.coin, e.usdt), &e.pool.reference))
}

/// Решение 403: задан ли предел мелких пулов (> 0) в опубликованном снимке — допуск мелкой стороны к котировке
/// (quote/select.rs) без чтения настройки. Ошибок нет.
pub fn on_snap(reg: &Registry) -> bool {
    reg.shallow_memo().with(|s| s.limit > 0)
}
