//! Файл: venues/bsc_dex/quote/bound_set.rs
//! Что делает: решение 361 — входы верха курса стороны DEX (quote/bound.rs): налоги монеты, предел хопов H, набор пулов
//! и поколения их состояния (ключ памяти). Набор — ВСЕ пулы реестра пар из {монета, USDT, активные промежуточные} (как
//! signal_epoch.rs state — шире, чем допуск котировки: недопущенный пул только ослабляет верх); при H ≤ 2 пулы между
//! двумя промежуточными не берутся (маршрут их не проходит). Предел хопов: классика (direct / smart) — via.rs
//! CLASSIC_MAX_HOPS (ревью t162, Н8: одна константа с кодом классики — путь через промежуточную собирается массивом этой
//! длины, классика длиннее не соберётся без смены константы, а с ней сменится и H границы); dex.router_mode = graph —
//! max(классика, dex.route_max_hops). Выделено из bound.rs (ревью t162, Н10: файл ≤ 200 строк).
//! Кто вызывает: quote/bound.rs, тесты (pool_set, hop_limit).
//! От чего зависит: parts.rs (реестр, проверки), pools/registry.rs (by_pair, pair_marks), quote/via.rs
//! (CLASSIC_MAX_HOPS), core/types.
//! Настройки: dex.router_mode, dex.route_max_hops [bsc_dex].

use std::collections::HashSet;

use alloy_primitives::Address;

use crate::core::types::{CheckState, PoolRef};

use super::super::ctx::Ctx;
use super::super::parts::Parts;
use super::super::pools::registry::Entry;
use super::via::CLASSIC_MAX_HOPS;

/// Что делает: налоги монеты из проверки известны и не отрицательны (налог < 0 — keep > 1, верх не строгий).
/// Принимает: части адаптера, монету. Ошибки: причина.
pub fn coin_taxes_ok(p: &Parts, coin: Address) -> Result<(), String> {
    match p.checks.coin(&coin).map(|c| c.taxes) {
        Some(CheckState::Known(t)) => {
            if t.buy_pct.is_sign_negative()
                || t.sell_pct.is_sign_negative()
                || t.transfer_pct.is_sign_negative()
            {
                return Err("налог монеты отрицательный".into());
            }
            Ok(())
        }
        _ => Err("налоги монеты не измерены".into()),
    }
}

/// Что делает: предел хопов маршрута: классика — CLASSIC_MAX_HOPS; режим graph — max(классика, dex.route_max_hops).
/// Принимает: окружение. Возвращает: H. Ошибки: режим graph без предела.
pub fn hop_limit(ctx: &Ctx) -> Result<usize, String> {
    if ctx.text("dex.router_mode").ok().as_deref() != Some("graph") {
        return Ok(CLASSIC_MAX_HOPS);
    }
    let h = ctx.int("dex.route_max_hops").map_err(|e| e.message)?;
    Ok(usize::try_from(h)
        .unwrap_or(usize::MAX)
        .max(CLASSIC_MAX_HOPS))
}

/// Пары токенов набора (обе ориентации реестра — вызывающий): при H ≤ 2 без пар двух промежуточных. Ошибок нет.
fn pairs(coin: Address, usdt: Address, mids: &[Address], hops: usize) -> Vec<(Address, Address)> {
    let mut tokens = vec![coin, usdt];
    tokens.extend(mids.iter().copied());
    let mut out = Vec::new();
    for (i, a) in tokens.iter().enumerate() {
        for b in &tokens[i + 1..] {
            if hops < 3 && i >= 2 {
                continue;
            }
            out.push((*a, *b));
        }
    }
    out
}

/// Что делает: пулы набора (копии записей реестра), без повторов. Принимает: части, монету, USDT, промежуточные, H.
/// Возвращает: записи. Ошибок нет.
pub fn pool_set(
    p: &Parts,
    coin: Address,
    usdt: Address,
    mids: &[Address],
    hops: usize,
) -> Vec<Entry> {
    let reg = &p.registry;
    let mut seen: HashSet<PoolRef> = HashSet::new();
    let mut out = Vec::new();
    for (a, b) in pairs(coin, usdt, mids, hops) {
        for e in reg.by_pair(a, b).into_iter().chain(reg.by_pair(b, a)) {
            if seen.insert(e.pool.reference.clone()) {
                out.push(e);
            }
        }
    }
    out
}

/// Что делает: поколения состояния пулов набора (registry pair_marks — без копий записей), по возрастанию, без повторов
/// (поколение уникально на запись реестра). Принимает: как pool_set. Возвращает: поколения. Ошибок нет.
pub fn gens(p: &Parts, coin: Address, usdt: Address, mids: &[Address], hops: usize) -> Vec<u64> {
    let reg = &p.registry;
    let mut out: Vec<u64> = Vec::new();
    for (a, b) in pairs(coin, usdt, mids, hops) {
        out.extend(reg.pair_marks(a, b));
        out.extend(reg.pair_marks(b, a));
    }
    out.sort_unstable();
    out.dedup();
    out
}
