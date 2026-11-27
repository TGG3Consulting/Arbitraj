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
//! Настройки: money.max_per_trade_usd [площадка DEX; решение 242 — DEX · «DEX ↔ DEX», venue_dir.rs], money.max_per_coin_usd, dry.budget_usdt (сухой режим),
//! dex_dex.size_step_base, dex_dex.min_profit_usd, exec.dex_slippage_pct [площадка DEX], size.bisect_steps,
//! size.grid_points, size.refine_steps, size.tolerance_pct, size.max_evals; size.parallel_points, size.bisect_probes
//! (решение 336, необязательные); size.early_exit_margin_usd (решение 337, необязательная); size.pool_breaks_max
//! (решение 344, необязательная).

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{Asset, CycleQuote, DexCycle, Direction, VenueId};
use crate::gateway::cpu_budget::{self, Tier};
use crate::settings::read::At;

use super::cycle_curve::CycleCurve;
use super::engine::{Core, lk};
use super::size::{Params, SizeChoice};

/// План сделки цикла — как он был в момент решения (в сделку — Deal.plan, JSON).
#[derive(Debug, Clone, Serialize)]
pub struct CyclePlan {
    pub cycle: DexCycle,
    pub account: crate::core::types::DexAccountId,
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
pub fn direction(core: &Core, c: &DexCycle) -> Result<Direction, String> {
    Ok(Direction {
        asset: Asset {
            chain: super::chain::tag_of_dex(core, &c.venue)?,
            ticker: c.base_ticker.clone(),
            contract: Some(c.base),
        },
        buy_on: c.venue.clone(),
        sell_on: c.venue.clone(),
    })
}

/// Восстанавливает сеть незакрытого цикла из двух совпадающих площадок направления и плана.
pub(super) fn restore_chain(d: &mut super::deal::Deal) {
    use super::deal::Step;
    if d.direction.asset.chain.is_some()
        || matches!(d.step, Step::Closed | Step::TakenOver)
        || d.direction.buy_on != d.direction.sell_on
    {
        return;
    }
    let Some(p) = crate::settings::evm_profiles::profile(&d.direction.buy_on.0) else {
        return;
    };
    let Some(c) = d.plan.get("cycle") else { return };
    if c.get("venue").and_then(serde_json::Value::as_str) == Some(p.venue)
        && let Some(token) = c
            .get("base")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| s.parse::<alloy_primitives::Address>().ok())
        && d.direction.asset.contract == Some(token)
    {
        d.direction.asset.chain = Some(crate::core::types::ChainId(p.chain.into()));
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
    if super::settlement_market::configured(core, v)?.is_some() {
        let quote = s.text("venue.quote_asset", at).map_err(|e| e.message)?;
        super::settlement::require_same(&quote, crate::settings::evm_profiles::LEDGER_QUOTE)?;
    }
    let min_usd = dec("dex_dex.min_profit_usd", at)?;
    let step = dec("dex_dex.size_step_base", at)?;
    let slip = dec("exec.dex_slippage_pct", at)?;
    let budget = match budget {
        Some(b) => b,
        None => limits(core, c, v)?,
    };
    // Решение 132: кошелёк — строки «DEX ↔ DEX» маппинга; нет строки — цикл не исполняется (NO_WALLET).
    let d = direction(core, c)?;
    let account = super::dex_account::of_direction(&core.inner.deps, &d)
        .await?
        .ok_or_else(|| super::dex_account::NO_WALLET.to_string())?;
    let venue = core
        .inner
        .deps
        .gw
        .get_as(v, Some(&account))
        .await
        .map_err(|e| e.message)?;
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
    // Решение 336: параллельные точки и пробы границ — те же необязательные size.*, что у кандидатов.
    let (par_points, bisect_probes, par_blocked) = super::size_par::par_settings(s)?;
    let mut p = Params {
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
        par_points,
        bisect_probes,
        early_floor: None,
        // Ревью t157 Б2: у цикла план строится прямо из точки подбора (финальной котировки нет) — без повторного
        // использования допуска пулов: точка цикла всегда по полному допуску (gateway/calc_share.rs).
        reuse: false,
        par_blocked,
        // Задача t159 (ревью t159 Н7): у цикла порога % нет (342 неприменимо); участков 343 нет — бирж нет, вложение =
        // вход × курс (монотонно), «слишком большой» — шаг пути не исполним целиком (cycle_curve.rs): путь цикла один,
        // вход каждого шага не убывает с входом цикла ⇒ нехватка ёмкости шага монотонна, бисекция точна (иные причины
        // NoRoute у адаптера тоже идут в TooLarge — их монотонность не доказана, docs/TASKS.md); мягкого срока 345
        // нет — у плана цикла нет внешнего
        // общего срока (cycle_watch.rs); изломы 344 — первого пула (cycle_curve.rs breaks).
        ext: super::size_extra::Extra {
            min_usd: Some(min_usd),
            budget_monotone: true,
            breaks_max: super::size_extra::flags(s).breaks_max,
            ..Default::default()
        },
    };
    // Решение 337: пол раннего выхода — порог dex_dex.min_profit_usd минус size.early_exit_margin_usd.
    p.early_floor = super::size_early::floor(s, min_usd);
    let mut curve = CycleCurve {
        venue,
        cycle: c.clone(),
    };
    // Решение 338 (ревью t157 Н1): план цикла — ярус счёта «кандидат» (котировки цикла в потоке счёта при параллельных
    // точках — venue.rs; без яруса они шли бы «фоном»).
    let search = super::size_search::search(&mut curve, &p);
    let found = cpu_budget::scope(Tier::Calc, search)
        .await
        .map_err(|f| format!("подбор входа цикла: {f}"))?;
    if found.early {
        let m = super::size_early::margin(s);
        return Err(super::size_early::reason(
            found.best.profit,
            min_usd,
            m,
            "подбор входа цикла",
        ));
    }
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
        account,
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
    // Решение 242: значение пары «площадка + тип направления» (DEX · «DEX ↔ DEX»), иначе площадка DEX, иначе система.
    let g = |n: &str, venue: Option<&str>, vd| {
        s.decimal_dir(n, vd, At { venue, coin: None })
            .map_err(|e| e.message)
    };
    let dir = direction(core, c)?;
    let per_trade = g(
        "money.max_per_trade_usd",
        Some(&v.0),
        super::venue_dir::of(core, &dir),
    )?;
    let per_coin = g("money.max_per_coin_usd", None, None)?;
    let asset = dir.asset;
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
