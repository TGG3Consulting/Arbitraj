//! Файл: core/cycle_plan.rs
//! Что делает: план сделки цикла DEX ↔ DEX (решение 124) — то же, что кандидат BingX ↔ DEX проходит в ядре, но без
//! перевода: (1) бюджет по лимитам money.* (на сделку по площадке DEX, остаток на базовую монету с учётом открытых
//! сделок; сухой режим — ещё dry.budget_usdt; ручная проверка может задать свой) и по свободной базовой монете кошелька
//! (Venue::dex_cycle_quote с кошельком — один запрос к узлу); (2) размер — общий поиск объёма ядра (size_search.rs,
//! решение 66) с кривой цикла (cycle_curve.rs), шаг — dex_dex.size_step_base, параметры — size.*; (3) порог: чистая
//! прибыль (все затраты решения 85: комиссии пулов, проскальзывание, газ свопов и разрешения, чаевые) ≥
//! dex_dex.min_profit_usd; (4) граница minOut = вход + затраты + порог, не ниже ожидаемого × (1 − exec.dex_slippage_pct)
//! (как минимум обмена ноги — наибольшее из «по границе» и «по допуску»). У исполнителя ArbExecutor это его minOut.
//! Кто вызывает: core/cycle_watch.rs (живой и сухой режим), core/cycle_dry.rs (ручная сухая сделка цикла).
//! От чего зависит: core/{cycle_curve, size, size_search, dry, engine, coin_key}.rs, gateway, settings.
//! Настройки: money.max_per_trade_usd [площадка DEX], money.max_per_coin_usd, dry.budget_usdt (сухой режим),
//! dex_dex.size_step_base, dex_dex.min_profit_usd, exec.dex_slippage_pct [площадка DEX], size.bisect_steps,
//! size.grid_points, size.refine_steps, size.tolerance_pct, size.max_evals.

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{Asset, CycleQuote, DexCycle, Direction, VenueId};
use crate::settings::read::At;

use super::cycle_curve::CycleCurve;
use super::engine::{Core, lk};
use super::size::{Params, SizeChoice};

/// План сделки цикла — как он был в момент решения (в сделку — Deal.plan, JSON).
#[derive(Debug, Clone, Serialize)]
pub struct CyclePlan {
    pub cycle: DexCycle,
    /// Вход (базовая) и расчёт на нём.
    pub input: Decimal,
    pub quote: CycleQuote,
    /// Граница выхода (базовая): вход + затраты + порог, не ниже ожидаемого × (1 − допуск).
    pub min_out: Decimal,
    /// Чистая прибыль в USDT (по курсу базовой).
    pub profit_usd: Decimal,
    /// Бюджет USDT и свободная базовая у кошелька.
    pub budget_usd: Decimal,
    pub wallet_base: Decimal,
    pub size: SizeChoice,
}

/// Направление сделки цикла в учёте ядра: базовая монета, покупка и продажа — на одной площадке DEX. Ошибок нет.
pub fn direction(c: &DexCycle) -> Direction {
    Direction {
        asset: Asset {
            ticker: c.base_ticker.clone(),
            bsc_contract: Some(c.base),
        },
        buy_on: c.venue.clone(),
        sell_on: c.venue.clone(),
    }
}

/// План. Принимает: ядро, цикл, бюджет USDT (None — по лимитам money.* и режиму). Возвращает: план.
/// Ошибки: текст причины (настройка не задана, нет бюджета, нет баланса, ниже порога, расчёт невозможен).
pub async fn plan(core: &Core, c: &DexCycle, budget: Option<Decimal>) -> Result<CyclePlan, String> {
    let s = &core.inner.deps.settings;
    let v = &c.venue;
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    let dec = |n: &str, a: At| s.decimal(n, a).map_err(|e| e.message);
    let min_usd = dec("dex_dex.min_profit_usd", at)?;
    let step = dec("dex_dex.size_step_base", at)?;
    let slip = dec("exec.dex_slippage_pct", at)?;
    let budget = match budget {
        Some(b) => b,
        None => limits(core, c, v)?,
    };
    // Решение 132: кошелёк — строки «DEX ↔ DEX» маппинга; нет строки — цикл не исполняется (NO_WALLET).
    let venue = super::dex_account::venue_dir(&core.inner.deps, &direction(c), v).await?;
    // Один запрос с кошельком: курсы, форма исполнения, свободная базовая (потолок входа).
    let probe = venue
        .dex_cycle_quote(c, c.input, true)
        .await
        .map_err(|e| e.message)?;
    let wallet = probe.wallet_base.unwrap_or_default();
    if wallet <= Decimal::ZERO {
        return Err(format!("у кошелька нет {} — вход цикла 0", c.base_ticker));
    }
    let cap = budget.min(wallet * probe.base_usdt);
    let int = |n: &str| -> Result<u32, String> {
        let x = s.integer(n, At::default()).map_err(|e| e.message)?;
        u32::try_from(x).map_err(|_| format!("{n} = {x}: вне пределов"))
    };
    let p = Params {
        step,
        budget: cap,
        guess: cap / probe.base_usdt,
        buy_breaks: Vec::new(),
        sell_breaks: Vec::new(),
        bisect_steps: int("size.bisect_steps")?,
        grid_points: int("size.grid_points")?,
        refine_steps: int("size.refine_steps")?,
        max_evals: int("size.max_evals")?,
        tolerance_pct: dec("size.tolerance_pct", At::default())?,
    };
    let mut curve = CycleCurve {
        venue,
        cycle: c.clone(),
    };
    let found = super::size_search::search(&mut curve, &p)
        .await
        .map_err(|f| format!("подбор входа цикла: {f}"))?;
    let q = found.best.data;
    // Порог в USDT (решение 124): одна настройка для базовых USDT и WBNB; в базовую — по курсу пулов адаптера.
    let profit_usd = q.net_base * q.base_usdt;
    if profit_usd < min_usd {
        return Err(format!(
            "лучший вход {} {}: чистая прибыль {} (≈ {} USDT) ниже порога dex_dex.min_profit_usd {min_usd}",
            q.input,
            c.base_ticker,
            q.net_base,
            profit_usd.round_dp(6)
        ));
    }
    let by_profit = q.input + q.cost_base + min_usd / q.base_usdt;
    let by_slip = q.output * (Decimal::ONE - slip / Decimal::ONE_HUNDRED);
    let mut size = found.choice;
    size.why = format!(
        "наибольшая чистая прибыль цикла {} {} на входе {} (бюджет {cap} USDT)",
        q.net_base, c.base_ticker, q.input
    );
    Ok(CyclePlan {
        cycle: c.clone(),
        input: q.input,
        min_out: by_profit.max(by_slip),
        profit_usd,
        budget_usd: cap,
        wallet_base: wallet,
        quote: q,
        size,
    })
}

/// Бюджет по лимитам: на сделку (по площадке DEX), остаток на базовую монету, в сухом режиме — dry.budget_usdt.
/// Ошибки: не задан лимит, лимит исчерпан.
fn limits(core: &Core, c: &DexCycle, v: &VenueId) -> Result<Decimal, String> {
    let s = &core.inner.deps.settings;
    let g = |n: &str, venue: Option<&str>| {
        s.decimal(n, At { venue, coin: None })
            .map_err(|e| e.message)
    };
    let per_trade = g("money.max_per_trade_usd", Some(&v.0))?;
    let per_coin = g("money.max_per_coin_usd", None)?;
    let asset = direction(c).asset;
    let used = lk(&core.inner.exposure)
        .per_coin
        .get(&asset)
        .copied()
        .unwrap_or_default();
    let mut cap = per_trade.min(per_coin - used);
    if let super::dry::Mode::Dry(b) = super::dry::mode(s) {
        cap = cap.min(b?);
    }
    if cap <= Decimal::ZERO {
        return Err(format!(
            "нет бюджета: на сделку {per_trade}, остаток на {} {}",
            c.base_ticker,
            per_coin - used
        ));
    }
    Ok(cap)
}
