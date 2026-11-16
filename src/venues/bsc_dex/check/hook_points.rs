//! Файл: venues/bsc_dex/check/hook_points.rs
//! Что делает: решение «335 (лесенка влияния)» — суммы пробы влияния крючка B/C в цикле крючков (check/hooks_head.rs)
//! по каждой стороне пула, без константных чисел — из данных:
//! - нижняя сумма — signal.probe_usd (объём проверки сигнала, решение 260: меньше её котировки сигнала не считаются);
//! - верхняя — min(наш максимум на сделку money.max_per_trade_usd по направлениям с DEX, ликвидность стороны пула в
//!   полосе dex.depth_band_pct) — как сумма сверки пулов без крючка (check/quoter_amount.rs, решение 261);
//! - число точек — hooks.impact_points; точки — геометрическая лесенка от нижней до верхней (мелкие суммы гуще:
//!   влияние крючка растёт с суммой, запись узла 17.11 — GAIB 0 / 0,0990 / 0,6735 % на 1 / 20 / 100 USDT).
//!   Верхняя не больше нижней — одна точка (верхняя); ликвидности стороны нет — одна точка на нижней сумме (как
//!   прежняя проба на одной сумме: квотер скажет, хватает ли ликвидности).
//!
//! Кто вызывает: check/hooks_head.rs (probe).
//! От чего зависит: check/quoter_amount.rs (max_trade_usd), pools/depth_book.rs (глубина стороны), pools/listen.rs.
//! Настройки: hooks.impact_points, signal.probe_usd [bsc_dex]; через вызываемые — money.max_per_trade_usd,
//! dex.depth_band_pct.

use rust_decimal::{Decimal, MathematicalOps};

use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::quoter_amount::{self, Fail};

/// Настройка: число точек лесенки влияния крючка по стороне.
pub const POINTS: &str = "hooks.impact_points";

/// Что делает: геометрическая лесенка сумм (чистая, см. шапку). Принимает: нижнюю и верхнюю сумму USDT, число точек.
/// Возвращает: суммы по возрастанию (до цента, без повторов; последняя — верхняя). Ошибок нет.
pub fn ladder_usd(lo: Decimal, hi: Decimal, n: u64) -> Vec<Decimal> {
    if hi <= Decimal::ZERO {
        return if lo > Decimal::ZERO {
            vec![lo]
        } else {
            Vec::new()
        };
    }
    if n <= 1 || hi <= lo || lo <= Decimal::ZERO {
        return vec![hi];
    }
    let steps = Decimal::from(n - 1);
    let ratio = (hi / lo).checked_powd(Decimal::ONE / steps);
    let mut out: Vec<Decimal> = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
    for i in 0..n - 1 {
        let v = match ratio {
            Some(r) => r.checked_powd(Decimal::from(i)).map(|k| lo * k),
            None => None,
        }
        .unwrap_or_else(|| lo + (hi - lo) * Decimal::from(i) / steps)
        .round_dp(2);
        if v < hi && out.last().is_none_or(|l| *l < v) {
            out.push(v);
        }
    }
    out.push(hi);
    out
}

/// Что делает: суммы лесенки покупки и продажи пула (см. шапку). Принимает: пулы, запись из памяти, знаки USDT.
/// Возвращает: (суммы покупки, суммы продажи), USDT. Ошибки: причина и имя незаданной настройки.
pub fn sides(p: &Pools, e: &Entry, usdt_dec: u8) -> Result<(Vec<Decimal>, Vec<Decimal>), Fail> {
    let n = p
        .ctx
        .int(POINTS)
        .map_err(|err| (err.message, err.setting))?;
    let lo = p
        .ctx
        .dec("signal.probe_usd")
        .map_err(|err| (err.message, err.setting))?;
    let max = quoter_amount::max_trade_usd(p)?;
    let d = p
        .reg
        .depth()
        .depth(&p.ctx, &p.reg, &p.tokens, e, usdt_dec)
        .map_err(|why| {
            (
                why,
                super::super::pools::depth_book::missing_setting(&p.ctx),
            )
        })?;
    Ok((
        ladder_usd(lo, max.min(d.band.buy), n),
        ladder_usd(lo, max.min(d.band.sell), n),
    ))
}
