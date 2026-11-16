//! Файл: venues/bsc_dex/check/hook_points.rs
//! Что делает: решение «335 (лесенка влияния)» — суммы пробы влияния крючка B/C в цикле крючков (check/hooks_head.rs)
//! по каждой стороне пула, без константных чисел — из данных:
//! - нижняя сумма — наименьший signal.probe_usd по областям направлений с переводом и DEX (ревью Н5: те же области,
//!   что читает сигнал, решение 260; check/quoter_amount.rs probe_low), не больше верхней; не задан ни у одного —
//!   сигнал считает на потолке, и лесенка — одна точка на верхней сумме;
//! - верхняя — min(наш максимум на сделку money.max_per_trade_usd по направлениям с DEX, ликвидность стороны пула в
//!   полосе dex.depth_band_pct) — как сумма сверки пулов без крючка (check/quoter_amount.rs, решение 261). Ревью Н12:
//!   ликвидность — по записи пула, прочитанной на блоке пробы (quote/depth_band.rs, без памяти глубины). Ревью Б1 (б):
//!   у продажи верх — монеты, а их на тот же бюджет больше, чем по цене пула (в «биржа → DEX» монеты куплены дешевле):
//!   сумма продажи делится на (1 − полоса/100) — столько монет стоит верх по худшей цене полосы. Монет больше этого —
//!   цена биржи ниже нижней границы полосы; такую сумму котировка не измерит, а сделка всё равно измерит пробой на
//!   своей сумме (exec/hook_trade.rs);
//! - число точек — hooks.impact_points; ревью Н3: первый замер — ⌈n/2⌉ точек (не меньше 2 при n ≥ 2) геометрической
//!   лесенкой от нижней до верхней (мелкие суммы гуще: запись узла 17.11 — GAIB 0 / 0,0990 / 0,6735 % на 1 / 20 /
//!   100 USDT), остальные n − ⌈n/2⌉ — доуточнение там, где соседние точки различаются больше всего
//!   (check/hook_fold.rs refine). Верхняя не больше нижней — одна точка (верхняя);
//! - ревью Н6: hooks.impact_points не задана — одна точка на check.probe_usdt (как до лесенки) и `flat`: итог —
//!   одно число на все суммы (check/hooks_head.rs убирает точки), пулы B/C торгуются, как раньше.
//!
//! Кто вызывает: check/hooks_head.rs (probe).
//! От чего зависит: check/quoter_amount.rs (max_trade_usd, probe_low), quote/depth_band.rs, pools/listen.rs.
//! Настройки: hooks.impact_points (необязательна, ревью Н6), check.probe_usdt (без неё), dex.depth_band_pct
//! [bsc_dex]; через вызываемые — money.max_per_trade_usd, signal.probe_usd.

use rust_decimal::{Decimal, MathematicalOps};

use crate::gateway::errors::ErrorCode;

use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::quote::depth_band;
use super::quoter_amount::{self, Fail};

/// Настройка: число точек лесенки влияния крючка по стороне.
pub const POINTS: &str = "hooks.impact_points";

/// Суммы замера по сторонам.
#[derive(Debug, Clone, PartialEq)]
pub struct Sides {
    /// Суммы первого замера покупки, USDT (по возрастанию).
    pub buy: Vec<Decimal>,
    /// Суммы первого замера продажи, USDT.
    pub sell: Vec<Decimal>,
    /// Сколько точек стороны — на доуточнение (ревью Н3).
    pub extra: usize,
    /// hooks.impact_points не задана — одно число на все суммы (ревью Н6).
    pub flat: bool,
}

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
        // Ревью 3 (Н-к): сумма, округлённая до 0, — не точка (вход 0).
        if v > Decimal::ZERO && v < hi && out.last().is_none_or(|l| *l < v) {
            out.push(v);
        }
    }
    out.push(hi);
    out
}

/// Число точек первого замера из n (ревью Н3, чистая): ⌈n/2⌉, не меньше min(n, 2). Ошибок нет.
pub fn first_round(n: u64) -> u64 {
    n.div_ceil(2).max(n.min(2))
}

/// Верх продажи по худшей цене полосы (ревью Б1 б, чистая): сумма / (1 − полоса/100). Полоса вне (0, 100) — как есть.
/// Ошибок нет.
pub fn widen(usd: Decimal, band_pct: Decimal) -> Decimal {
    let keep = Decimal::ONE - band_pct / Decimal::ONE_HUNDRED;
    if keep <= Decimal::ZERO || keep > Decimal::ONE {
        return usd;
    }
    usd.checked_div(keep).unwrap_or(usd)
}

/// Что делает: суммы замера покупки и продажи пула (см. шапку). Принимает: пулы, запись пула на блоке пробы, знаки
/// USDT. Возвращает: Sides. Ошибки: причина и имя незаданной настройки.
pub fn sides(p: &Pools, fresh: &Entry, usdt_dec: u8) -> Result<Sides, Fail> {
    let set = |err: crate::gateway::errors::GatewayError| (err.message, err.setting);
    let n = match p.ctx.int(POINTS) {
        Ok(n) => n,
        Err(err) if err.code == ErrorCode::NotConfigured => {
            let v = p.ctx.dec("check.probe_usdt").map_err(set)?;
            return Ok(Sides {
                buy: vec![v],
                sell: vec![v],
                extra: 0,
                flat: true,
            });
        }
        Err(err) => return Err(set(err)),
    };
    let max = quoter_amount::max_trade_usd(p)?;
    let lo = quoter_amount::probe_low(p)?.unwrap_or(max).min(max);
    let band_pct = p.ctx.dec("dex.depth_band_pct").map_err(set)?;
    let units = depth_band::band_units(fresh, band_pct).map_err(|why| (why, None))?;
    let band = depth_band::band_from_units(&p.ctx, &p.reg, &p.tokens, fresh, usdt_dec, units)
        .map_err(|why| (why, None))?;
    let first = first_round(n);
    let sell_hi = widen(max.min(band.sell), band_pct);
    Ok(Sides {
        buy: ladder_usd(lo, max.min(band.buy), first),
        sell: ladder_usd(lo, sell_hi, first),
        extra: usize::try_from(n - first).unwrap_or(0),
        flat: false,
    })
}
