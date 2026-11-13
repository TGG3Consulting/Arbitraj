//! Файл: core/profit.rs
//! Что делает: решение по кандидату (шаг 4 ТЗ, решение 66): сначала подбирает объём с наибольшей прибылью в долларах
//! после всех затрат при условии «вложение ≤ бюджета» (size_search.rs по кривой size_curve.rs — полный расчёт
//! costs.rs на каждый объём), и только потом проверяет пороги: прибыль ≥ money.min_profit_usd и ≥ money.min_profit_pct
//! от вложенного. Иначе — отказ с причиной и лучшим расчётом (для подбора порогов). В расчёт кладётся объяснение
//! выбора: «оптимум 7.3 WFI: прибыль 0.41; на бюджете 20 (9.75 WFI) — 0.33».
//! Кто вызывает: core/candidates.rs (очередь тяжёлого расчёта), core/precheck.rs (пересчёт перед покупкой).
//! От чего зависит: costs.rs, dirctx.rs, limits.rs, size_curve.rs, size_search.rs, market_state.rs, settings.
//! Решение 95: найденный оптимум пересчитывается ФИНАЛЬНОЙ котировкой (gateway/quote_stage.rs) — у DEX при
//! exec.via_kyber это цена KyberSwap (тот, кто исполнит); пороги проверяются по ней. Отказ финальной котировки —
//! отказ кандидата с причиной. Решение 111: исполнитель даёт маршрут не на весь объём — пересчёт на исполнимый объём.
//! Решение 129: этапы расчёта замеряются (calc_timing.rs): контекст направления, бюджет, подбор объёма, финальная
//! котировка — по ним отказ по сроку называет, где расчёт стоял.
//! Решение 175: сначала — делистинг монеты на сторонах направления (delist_check.rs, delist.block_hours).
//! Решение 253 (г): затем — входы по монете с биржи покупки не закрыты отказом вывода (withdraw_block.rs).
//! Решение 234 (п. 3): затем — свежесть замера налогов на DEX-сторонах (tax_age.rs, tax.max_age_s).
//! Решение 246: кривая подбора знает бюджет — объём сверх бюджета отбрасывается до котировок B (costs.rs evaluate_capped).
//! Решение 265: этапами подробного замера (calc_detail.rs, только при log.step_timing) стали и проверки до расчёта (делистинг, закрытые входы, свежесть налогов) и исполнимый
//! объём у исполнителя в финальной котировке — их время видно в «Время по шагам» предпроверки (calc_steps.rs).
//! Решение 242: пороги прибыли — с областью «площадка + тип направления» (venue_dir.rs thresholds), не задано — как раньше.
//! Настройки: money.min_profit_usd, money.min_profit_pct [вся система, площадка + тип направления], tax.max_age_s (tax_age.rs), kyber.final_gate_usd (решение 111; только при exec.via_kyber =
//! да на DEX направления — решение 121 B5, через gateway/quote_stage.rs), size.* (+ всё, что читают costs.rs, dirctx.rs, limits.rs).

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{Direction, Side};
use crate::settings::read::At;

use super::calc_detail::{stage_detail, stage_sync_detail};
use super::calc_timing::{stage, stage_sync};
use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::{Core, rd};
use super::size::SizeChoice;
use super::size_curve::{CoreCurve, params};

/// Итог расчёта кандидата.
#[derive(Debug, Clone, Serialize)]
pub enum Verdict {
    /// Прибыль выше порога — сделка.
    Accept(Calc),
    /// Отказ: причина и лучший найденный расчёт (если был).
    Reject { reason: String, best: Option<Calc> },
}

/// Расчёт кандидата. Принимает: ядро, направление, режим (зафиксирован вызывающим: очередь кандидатов — на расчёт,
/// предпроверка сделки — всегда Live, решение 74). Возвращает: Verdict. Ошибок нет — всё в Reject.
pub async fn decide(core: &Core, d: &Direction, mode: &super::dry::Mode) -> Verdict {
    decide_capped(core, d, mode, None).await
}

/// То же, но бюджет не больше `cap` (решение 103: свободный баланс A прямо перед ордером меньше бюджета — объём
/// подбирается заново под свободный остаток, refit.rs). Принимает: ядро, направление, режим, предел. Ошибок нет.
pub async fn decide_capped(
    core: &Core,
    d: &Direction,
    mode: &super::dry::Mode,
    cap: Option<Decimal>,
) -> Verdict {
    let reject = |reason: String, best: Option<Calc>| Verdict::Reject { reason, best };
    // Сухой режим без бюджета — сигналов нет, считать нечего (решение 74).
    if let super::dry::Mode::Dry(Err(e)) = mode {
        return reject(e.clone(), None);
    }
    // Решение 175: монета в делистинге на любой стороне ближе delist.block_hours — сделки нет (до любого расчёта).
    // Решение 265: проверки до расчёта — этапы подробного замера (calc_detail.rs; только при log.step_timing).
    let delist = super::delist_check::check(core, d, chrono::Utc::now());
    if let Err(e) = stage_detail(|| "делистинг".into(), delist).await {
        return reject(e, None);
    }
    // Решение 253 (г): входы по монете с биржи покупки закрыты после окончательного отказа вывода с неё.
    if let Err(e) = stage_sync_detail(
        || "закрытые входы после отказа вывода".into(),
        || super::withdraw_block::check(core, d),
    ) {
        return reject(e, None);
    }
    // Решение 234 (п. 3): замер налогов монеты на DEX-сторонах не старше tax.max_age_s (не задана — проверки нет).
    if let Err(e) = stage_sync_detail(
        || "свежесть замера налогов".into(),
        || super::tax_age::check(core, d, chrono::Utc::now()),
    ) {
        return reject(e, None);
    }
    let cx = match stage_sync(|| "контекст направления".into(), || DirCtx::build(core, d))
    {
        Ok(c) => c,
        Err(e) => return reject(e, None),
    };
    let s = &core.inner.deps.settings;
    // Решение 242: пороги — площадки и типа направления сделки (правило стороны), иначе вся система.
    let (min_usd, min_pct) = match super::venue_dir::thresholds(s, super::venue_dir::of(core, d)) {
        Ok(t) => t,
        Err(e) => return reject(e, None),
    };
    let budget = match stage(|| "бюджет".into(), super::limits::budget(core, &cx, mode)).await
    {
        Ok(b) => cap.map_or(b, |c| b.min(c)),
        Err(e) => return reject(e, None),
    };
    let price = rd(&core.inner.market).best(&d.buy_on, &cx.a.ticker, Side::Buy);
    let price = match price.or(rd(&core.inner.spreads)
        .get(d)
        .and_then(|v| v.prices.as_ref().map(|p| p.buy)))
    {
        Some(p) if !p.is_zero() => p,
        _ => return reject("нет цены покупки на A".into(), None),
    };
    let p = match params(core, &cx, budget, price, None) {
        Ok(p) => p,
        Err(e) => return reject(e, None),
    };
    // Решение 246: бюджет поиска — и у кривой: объём сверх него отбрасывается до котировок B (выбор тот же).
    let mut curve = CoreCurve {
        core,
        cx: &cx,
        src: None,
        budget: Some(p.budget),
    };
    let found = match stage(
        || "подбор объёма".into(),
        super::size_search::search(&mut curve, &p),
    )
    .await
    {
        Ok(f) => f,
        Err(e) => return reject(e.to_string(), None),
    };
    let mut choice = found.choice;
    choice.why = why(&choice, &d.asset.ticker);
    // Решение 111: фильтр финальной котировки (kyber.final_gate_usd). У направления с DEX при exec.via_kyber = да
    // финальная котировка — запрос к KyberSwap (лимит запросов); своя математика заведомо минусовая — не тратим запрос.
    // Решение 121 B5: при своём сборщике (exec.via_kyber = нет) фильтр не действует. 0 или не задано — без фильтра.
    if let Some(gate) = final_gate(core, d)
        && choice.profit < gate
    {
        return reject(
            format!(
                "своя математика {} USDT ниже фильтра финальной котировки kyber.final_gate_usd {gate} ({})",
                choice.profit.round_dp(4),
                choice.why
            ),
            Some(found.best.data),
        );
    }
    // Решение 95: объём найден дешёвыми котировками; оптимум пересчитывается ФИНАЛЬНОЙ котировкой (quote_stage) —
    // площадка отдаёт цену того, кто исполнит (у DEX при exec.via_kyber — KyberSwap). Вердикт — по ней.
    let fin = final_calc(core, &cx, choice.qty, budget, p.step);
    let mut c = match stage(|| "финальная котировка".into(), fin).await {
        Ok(f) => f,
        Err(e) => {
            return reject(
                format!("финальная котировка: {e} ({})", choice.why),
                Some(found.best.data),
            );
        }
    };
    // Финальная цена хуже — объём мог быть ужат до бюджета (final_calc): объём сделки — фактический.
    if c.qty != choice.qty {
        choice.why = format!(
            "{}; по финальной котировке объём ужат (бюджет или глубина исполнителя): {} {}",
            choice.why,
            c.qty.normalize(),
            d.asset.ticker
        );
        choice.qty = c.qty;
    }
    c.size = Some(choice.clone());
    if c.profit_usd < min_usd {
        return reject(
            format!(
                "прибыль {} USDT ниже порога {min_usd} ({})",
                c.profit_usd.round_dp(4),
                choice.why
            ),
            Some(c),
        );
    }
    if c.profit_pct < min_pct {
        return reject(
            format!(
                "прибыль {}% ниже порога {min_pct}% ({})",
                c.profit_pct.round_dp(4),
                choice.why
            ),
            Some(c),
        );
    }
    Verdict::Accept(c)
}

/// Финальный расчёт на найденном объёме (решение 95): тот же полный расчёт costs.rs, но внутри
/// gateway::quote_stage::final_scope — адаптер отдаёт котировку исполнителя. Списание по худшей цене не должно выйти за
/// бюджет: если финальная цена хуже и списание вышло за бюджет (объём подобран впритык к бюджету по своей цене) —
/// объём один раз ужимается пропорционально (qty × бюджет / списание, вниз до шага) и считается заново.
/// Принимает: ядро, контекст, объём, бюджет, шаг количества. Возвращает: Calc. Ошибки: текст (отказ расчёта, бюджет).
async fn final_calc(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    budget: Decimal,
    step: Decimal,
) -> Result<Calc, String> {
    let eval =
        |q: Decimal| crate::gateway::quote_stage::final_scope(super::costs::evaluate(core, cx, q));
    // Решение 111: исполнитель (KyberSwap) даёт маршрут не на весь объём — пересчёт на то, что он реально исполнит
    // (вниз до шага), а не отказ. Живой случай 02.11: RVV — 359317.97 из 360732.6 (99.6%), прибыль почти та же,
    // а кандидат отброшен. Пороги прибыли проверяются как обычно — уже на меньшем объёме.
    let (c, qty) = match eval(qty).await {
        Ok(c) => (c, qty),
        Err(super::size::Fail::TooLarge(t)) => {
            let fill = stage_detail(
                || "исполнимый объём у исполнителя".into(),
                final_fillable(core, cx, qty),
            )
            .await
            .map_err(|e| format!("{t}; {e}"))?;
            let q = if step > Decimal::ZERO {
                (fill / step).floor() * step
            } else {
                fill
            };
            if q <= Decimal::ZERO || q >= qty {
                return Err(t);
            }
            (eval(q).await.map_err(|f| f.to_string())?, q)
        }
        Err(f) => return Err(f.to_string()),
    };
    if c.spend_max <= budget {
        return Ok(c);
    }
    let over = |c: &Calc| {
        format!(
            "списание по худшей цене {} больше бюджета {budget}",
            c.spend_max.round_dp(4)
        )
    };
    if step <= Decimal::ZERO || c.spend_max <= Decimal::ZERO {
        return Err(over(&c));
    }
    let q = (qty * budget / c.spend_max / step).floor() * step;
    if q <= Decimal::ZERO || q >= qty {
        return Err(over(&c));
    }
    let c2 = eval(q).await.map_err(|f| f.to_string())?;
    if c2.spend_max > budget {
        return Err(over(&c2));
    }
    Ok(c2)
}

/// Порог фильтра финальной котировки (kyber.final_gate_usd), если в направлении есть DEX, чья финальная котировка —
/// у внешнего исполнителя (решение 121 B5: exec.via_kyber = да — спрашивает шлюз, gateway::quote_stage::final_external;
/// ядро адаптеры не знает). Свой сборщик — запросов к агрегатору нет, фильтр не нужен. None — 0, не задан, DEX нет
/// или внешнего исполнителя нет. Ошибок нет.
fn final_gate(core: &Core, d: &Direction) -> Option<Decimal> {
    let settings = &core.inner.deps.settings;
    let external = rd(&core.inner.kinds).iter().any(|(v, k)| {
        (v == &d.buy_on || v == &d.sell_on)
            && *k == crate::gateway::contract::VenueKind::Dex
            && crate::gateway::quote_stage::final_external(settings, v)
    });
    if !external {
        return None;
    }
    core.inner
        .deps
        .settings
        .decimal("kyber.final_gate_usd", At::default())
        .ok()
        .filter(|g| !g.is_zero())
}

/// Сколько исполнитель реально исполнит на обеих сторонах (финальная котировка; меньшее из двух). Решение 111.
/// Ошибки: текст — площадка не подключена, котировка не получена.
async fn final_fillable(core: &Core, cx: &DirCtx, qty: Decimal) -> Result<Decimal, String> {
    let d = &cx.dir;
    // Решение 132: DEX — кошелёк строки маппинга направления.
    let deps = &core.inner.deps;
    let a = super::dex_account::venue_dir(deps, d, &d.buy_on).await?;
    let b = super::dex_account::venue_dir(deps, d, &d.sell_on).await?;
    // Решение 211: каждой площадке — её тикер монеты (Presence::ticker в DirCtx).
    let (on_a, on_b) = (cx.on_a(), cx.on_b());
    let (qa, qb) = crate::gateway::quote_stage::final_scope(async {
        tokio::join!(
            a.quote(&on_a, Side::Buy, qty),
            b.quote(&on_b, Side::Sell, qty)
        )
    })
    .await;
    let qa = qa.map_err(|e| format!("{}: {}", d.buy_on.0, e.message))?;
    let qb = qb.map_err(|e| format!("{}: {}", d.sell_on.0, e.message))?;
    Ok(qa.fillable_qty.min(qb.fillable_qty))
}

/// Почему объём лучший — текст для журнала и «Кандидатов». Принимает: итог подбора, тикер. Ошибок нет.
pub fn why(c: &SizeChoice, ticker: &str) -> String {
    let r = |v: Decimal| v.round_dp(4).normalize();
    let head = format!(
        "оптимум {} {ticker}: прибыль {}",
        c.qty.normalize(),
        r(c.profit)
    );
    let budget = if c.qty == c.max_qty {
        format!(
            "это и есть наибольший объём в пределах бюджета {}",
            r(c.budget)
        )
    } else {
        format!(
            "на бюджете {} ({} {ticker}) — {}",
            r(c.budget),
            c.max_qty.normalize(),
            r(c.at_budget_profit)
        )
    };
    let mut exact = String::new();
    if !c.max_exact {
        exact.push_str("; граница бюджета не уточнена до шага (мало size.bisect_steps)");
    }
    if c.capped {
        exact.push_str("; поиск остановлен пределом size.max_evals — лучший из посчитанных");
    }
    format!(
        "{head}; {budget}; допустимо {}–{} {ticker}, посчитано объёмов {}, пиков {}{exact}",
        c.min_qty.normalize(),
        c.max_qty.normalize(),
        c.evals,
        c.peaks
    )
}
