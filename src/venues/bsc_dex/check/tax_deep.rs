//! Файл: venues/bsc_dex/check/tax_deep.rs
//! Что делает: есть ли у монеты пул для пробы налогов (решение 73): достоверный (has_valid) и глубокий в оба
//! направления (has_deep — как отбор пробы, taxes_util.rs probe_pools). По ним цикл налогов отличает «нет пула»,
//! «нет глубокого пула» (Why::Shallow — повтор по «пул готов», не по tax.retry_s) и сбой узла.
//! Кто вызывает: check/tax_pass.rs, check/tax_cycle.rs, check/taxes.rs (SHALLOW).
//! От чего зависит: pools/listen.rs (Pools), quote/select.rs (deep_side), tokens.rs.
//! Настройки: через quote/select.rs — dex.min_pool_liquidity_usd, dex.depth_band_pct, dex.depth_hysteresis_pct,
//! dex.depth_log_min_s [bsc_dex].

use super::super::pools::listen::Pools;

/// Причина «нет пула, глубокого в оба направления» — когда пробовать нечем, а по пулам причин нет.
pub const SHALLOW: &str = "нет достоверного пула, глубокого в оба направления (dex.min_pool_liquidity_usd в полосе dex.depth_band_pct), для пробного обмена";

/// Есть ли у монеты достоверный пул. Ошибок нет.
pub fn has_valid(p: &Pools, ticker: &str) -> bool {
    p.reg.by_ticker(ticker).iter().any(|e| e.valid)
}

/// Есть ли у монеты достоверный пул, глубокий в оба направления (решение 73; как отбор пробы — taxes_util.rs
/// probe_pools). Знаки USDT ещё не известны (ни одной пробы не было) — по достоверному пулу. Ошибок нет.
pub fn has_deep(p: &Pools, ticker: &str) -> bool {
    let dec = p
        .tokens
        .usdt(&p.ctx)
        .ok()
        .and_then(|u| p.tokens.decimals_known(u));
    let Some(dec) = dec else {
        return has_valid(p, ticker);
    };
    let both = super::super::quote::select::Need::Both;
    p.reg.by_ticker(ticker).iter().filter(|e| e.valid).any(|e| {
        matches!(
            super::super::quote::select::deep_side(&p.ctx, &p.reg, &p.tokens, e, dec, both),
            Ok(None)
        )
    })
}
