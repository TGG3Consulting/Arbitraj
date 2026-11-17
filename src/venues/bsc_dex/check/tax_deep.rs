//! Файл: venues/bsc_dex/check/tax_deep.rs
//! Что делает: есть ли у монеты пул для пробы налогов (решение 73; решение 390 — t178): достоверный (has_valid) и
//! годный для пробы (has_deep — ровно отбор пробы taxes_util.rs probe_pools: глубокий в оба направления или мелкий, где
//! пробный круг проходит). По ним цикл налогов отличает «нет пула»,
//! «нет глубокого пула» (Why::Shallow — повтор по «пул готов», не по tax.retry_s), «глубина не считается» и сбой узла
//! (оба — Why::Node, повтор через tax.retry_s).
//! Кто вызывает: check/tax_pass.rs, check/tax_cycle.rs, check/taxes.rs (SHALLOW).
//! От чего зависит: pools/listen.rs (Pools), check/taxes_util.rs (probe_pools), tokens.rs.
//! Решение 211: монета — адрес контракта (пулы монеты — по адресу, не по тикеру).
//! Настройки: через quote/select.rs — dex.min_pool_liquidity_usd, dex.depth_band_pct, dex.depth_hysteresis_pct,
//! dex.depth_log_min_s [bsc_dex]; через quote/side_fit.rs — check.probe_usdt, dex.quoter_max_diff_pct.

use alloy_primitives::Address;

use super::super::pools::listen::Pools;

/// Причина «нет пула, глубокого в оба направления» — когда пробовать нечем, а по пулам причин нет.
pub const SHALLOW: &str = "нет достоверного пула для пробного обмена: ни глубокого в оба направления (dex.min_pool_liquidity_usd в полосе dex.depth_band_pct), ни такого, где пробный круг на check.probe_usdt или её доле проходит (решение 390)";

/// Есть ли у монеты достоверный пул. Ошибок нет.
pub fn has_valid(p: &Pools, coin: &Address) -> bool {
    p.reg.by_coin(coin).iter().any(|e| e.valid)
}

/// Глубокий пул монеты (решение 73).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deep {
    /// Есть достоверный пул для пробы (глубокий в оба направления или с проходящим пробным кругом).
    Yes,
    /// Глубина посчитана у достоверных пулов — все мелкие и круг ни в одном не проходит (ждать «пул готов»).
    No,
    /// Глубокого нет, а у какого-то пула глубина не считается (нет курса промежуточной монеты и т. п.) — помеха, не
    /// вердикт: повтор через tax.retry_s.
    Unknown,
}

/// Есть ли у монеты пул для пробы (ровно отбор пробы — taxes_util.rs probe_pools). Знаки USDT ещё не известны (ни одной пробы
/// не было) — по достоверному пулу. Ошибок нет.
pub fn deep(p: &Pools, coin: &Address) -> Deep {
    let dec = p
        .tokens
        .usdt(&p.ctx)
        .ok()
        .and_then(|u| p.tokens.decimals_known(u));
    let Some(dec) = dec else {
        return if has_valid(p, coin) {
            Deep::Yes
        } else {
            Deep::No
        };
    };
    // Решение 390: тот же отбор, что у пробы (taxes_util.rs probe_pools): глубокий в оба направления или мелкий, где
    // пробный круг проходит.
    match super::taxes_util::probe_pools(&p.ctx, &p.reg, &p.tokens, coin, dec) {
        Ok((pools, _, _)) if !pools.is_empty() => Deep::Yes,
        Ok((_, env, _)) if env.is_empty() => Deep::No,
        Ok(_) | Err(_) => Deep::Unknown,
    }
}

/// Есть ли у монеты пул для пробы (deep == Yes). Ошибок нет.
pub fn has_deep(p: &Pools, coin: &Address) -> bool {
    deep(p, coin) == Deep::Yes
}
