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
//! Решение 266: пересчёт предпроверки при precheck.fast_recalc = да — сначала один финальный расчёт на объёме кандидата
//! (precheck_fast.rs recalc_boxed, после параметров подбора и до самого подбора); не прошёл — полный подбор, как раньше.
//! Решение 337: ранний выход подбора (size_early.rs, size.early_exit_margin_usd) — отказ без финальной котировки.
//! Задача t159 (ревью t159): решение 342 — порог % направления — ограничение подбора (в логике; size_pct.rs: лучший по
//! USDT среди проходящих оба порога; на объёме у границы % финальная не прошла % — один пересчёт на ближайшем меньшем
//! проходящем, size_retry.rs); решение 345 — мягкий срок size.soft_deadline_pct только при внешнем общем сроке
//! (decide_within: candidates.rs, busy_calc.rs; в предпроверке и refit его нет); решение 344 — изломы пулов DEX
//! маршрута — внутри подбора (size_breaks.rs, после быстрого пересчёта 266); size_extra.rs apply.
//! Решение 242: пороги прибыли — с областью «площадка + тип направления» (venue_dir.rs thresholds), не задано — как раньше.
//! Настройки: money.min_profit_usd, money.min_profit_pct [вся система, площадка + тип направления], tax.max_age_s (tax_age.rs), kyber.final_gate_usd (решение 111; только при exec.via_kyber =
//! да на DEX направления — решение 121 B5, через gateway/quote_stage.rs), size.* (+ всё, что читают costs.rs, dirctx.rs, limits.rs).

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{Direction, Side, VenueId};
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
    decide_capped(core, d, mode, None, None, None, None).await
}

/// То же под внешним общим сроком profit.calc_timeout_ms, начатым в `from` (решение 345, ревью t159 Б2: мягкий срок
/// подбора — только здесь; candidates.rs, busy_calc.rs). Принимает: ядро, направление, режим, начало срока. Ошибок нет.
pub async fn decide_within(
    core: &Core,
    d: &Direction,
    mode: &super::dry::Mode,
    from: std::time::Instant,
) -> Verdict {
    decide_capped(core, d, mode, None, None, None, Some(from)).await
}

/// То же, что decide, с уже построенным контекстом направления (ревью t162, Н7: отсев sieve.rs построил его — второй раз
/// не строится, σ та же). Принимает: ядро, направление, режим, контекст (None — строится, как в decide). Ошибок нет.
pub async fn decide_pre(
    core: &Core,
    d: &Direction,
    mode: &super::dry::Mode,
    pre: Option<Box<DirCtx>>,
) -> Verdict {
    decide_capped(core, d, mode, None, None, pre, None).await
}

/// То же, но бюджет не больше `cap` (решение 103: свободный баланс A прямо перед ордером меньше бюджета — объём
/// подбирается заново под свободный остаток, refit.rs). Принимает: ядро, направление, режим, предел, объём кандидата
/// (решение 266: пересчёт предпроверки при precheck.fast_recalc = да — precheck_fast.rs; None — как раньше), готовый
/// контекст направления (ревью t162, Н7: от отсева кандидата; None — строится здесь), начало внешнего общего срока
/// (решение 345; None — его нет: мягкого срока подбора нет). Ошибок нет.
pub async fn decide_capped(
    core: &Core,
    d: &Direction,
    mode: &super::dry::Mode,
    cap: Option<Decimal>,
    want: Option<&mut super::precheck_fast::Want>,
    pre: Option<Box<DirCtx>>,
    outer_from: Option<std::time::Instant>,
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
    if super::settlement_market::mixed(core, d) {
        return match Box::pin(super::settlement_entry::decide(
            core, d, mode, cap, outer_from,
        ))
        .await
        {
            Ok(c) => Verdict::Accept(c),
            Err(e) => reject(e, None),
        };
    }
    let cx = match pre {
        Some(c) => *c,
        None => match stage_sync(|| "контекст направления".into(), || DirCtx::build(core, d))
        {
            Ok(c) => c,
            Err(e) => return reject(e, None),
        },
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
    let mut p = match params(core, &cx, budget, price, None) {
        Ok(p) => p,
        Err(e) => return reject(e, None),
    };
    // Решение 337: пол раннего выхода подбора — порог прибыли направления минус size.early_exit_margin_usd.
    p.early_floor = super::size_early::floor(s, min_usd);
    // Задача t159: пороги — ограничение поиска (342), изломы пулов DEX (344, внутри подбора), мягкий срок (345).
    super::size_extra::apply(s, &mut p, (min_usd, min_pct), outer_from);
    // Решение 266: объём кандидата на свежих данных — один финальный расчёт; прошёл пороги — план без подбора, иначе
    // (ошибка, ниже порога, внешний исполнитель) — полный подбор ниже, как раньше. Будущее — в куче и собирается не
    // здесь (recalc_boxed): кадр расчёта кандидата не растёт; объём задан только у пересчёта предпроверки.
    let capture = want.is_none() && cap.is_none() && super::precheck_hint::enabled(core, &cx);
    if let Some(w) = want {
        let min = (min_usd, min_pct);
        let fast = super::precheck_fast::recalc_boxed(core, &cx, w, budget, p.step, min);
        if let Some(c) = fast.await {
            return Verdict::Accept(c);
        }
    }
    // Решение 246: бюджет поиска — и у кривой: объём сверх него отбрасывается до котировок B (выбор тот же).
    let mut curve = CoreCurve {
        core,
        cx: &cx,
        src: None,
        budget: Some(p.budget),
    };
    let stamp = capture
        .then(|| super::precheck_hint::stamp(core, &cx))
        .flatten();
    let found = match stage(
        || "подбор объёма".into(),
        // Предел стека (tests/core_dry_mode.rs precheck_replans_live): будущее подбора — в куче, не в кадре
        // decide_capped (в нём части t159: участки, изломы, порог %, мягкий срок).
        Box::pin(super::size_search::search_observed(
            &mut curve,
            &p,
            |memo, found| {
                if let Some(stamp) = stamp {
                    super::precheck_hint::remember(core, &cx, stamp, memo, found);
                }
            },
        )),
    )
    .await
    {
        Ok(f) => f,
        Err(e) => return reject(e.to_string(), None),
    };
    let mut choice = found.choice;
    choice.why = why(&choice, &d.asset.ticker);
    if choice.capped && choice.evals >= p.max_evals {
        choice
            .why
            .push_str("; достигнут предел числа расчётов size.max_evals");
    }
    // Ревью t157 Н3: size.parallel_points задана без cpu.threads_max — точки по одной; причина — в пояснении.
    if p.par_blocked {
        choice.why.push_str(super::size_par::PAR_BLOCKED);
    }
    if found.pct_moved {
        choice.why.push_str(&super::size_extra::pct_note(min_pct));
    }
    if found.late {
        choice.why.push_str(super::size_extra::LATE);
    }
    // Решение 337: ранний выход — уже по границам и сетке ниже порога с запасом: без уточнения и финальной котировки.
    if found.early {
        let margin = super::size_early::margin(s);
        let text = super::size_early::reason(choice.profit, min_usd, margin, &choice.why);
        return reject(text, Some(found.best.data));
    }
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
    let remaining = super::final_depth::before_buy()
        .filter(|_| !found.late && choice.evals < p.max_evals)
        .map(|hard| p.ext.deadline.map_or(hard, |soft| soft.min(hard)));
    let fin = super::final_depth::after_search(
        core,
        &cx,
        choice.qty,
        budget,
        p.step,
        (min_usd, min_pct),
        remaining,
    );
    let (mut c, recovered) = match stage(|| "финальная котировка".into(), Box::pin(fin)).await
    {
        Ok(f) => f,
        Err(e) => {
            return reject(
                format!("финальная котировка: {e} ({})", choice.why),
                Some(found.best.data),
            );
        }
    };
    if recovered {
        choice.why.push_str(
            "; после отказа по измеренной глубине меньший объём прошёл полный финальный расчёт",
        );
        choice.profit = c.profit_usd;
    }
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
        // Ревью t159 Н2: объём у границы порога % — один пересчёт на ближайшем меньшем проходящем (size_retry.rs).
        // Предел стека (tests/core_dry_mode.rs): расчёт c — в куче через ожидание, будущее пересчёта собирается в своём
        // кадре (size_retry.rs retry_boxed), не в кадре decide_capped.
        let c = Box::new(c);
        // Повторное ревью t159 Н-2: тот же фильтр kyber.final_gate_usd, что у основной финальной котировки (у
        // внешнего исполнителя пересчёт — ещё один запрос к нему).
        let gate = final_gate(core, d);
        if let Some(fb) = found.fallback.as_ref().map(|f| (f.qty, f.profit))
            && gate.is_none_or(|g| fb.1 >= g)
        {
            let min = (min_usd, min_pct);
            let retry =
                super::size_retry::retry_boxed(core, &cx, fb, (budget, p.step), min, &choice);
            if let Some(c2) = retry.await {
                return Verdict::Accept(*c2);
            }
        }
        return reject(
            format!(
                "прибыль {}% ниже порога {min_pct}% ({})",
                c.profit_pct.round_dp(4),
                choice.why
            ),
            Some(*c),
        );
    }
    Verdict::Accept(c)
}

/// Финальный расчёт на найденном объёме (решение 95): тот же полный расчёт costs.rs, но внутри
/// gateway::quote_stage::final_scope — адаптер отдаёт котировку исполнителя. Списание по худшей цене не должно выйти за
/// бюджет: если финальная цена хуже и списание вышло за бюджет (объём подобран впритык к бюджету по своей цене) —
/// объём один раз ужимается пропорционально (qty × бюджет / списание, вниз до шага) и считается заново.
/// Принимает: ядро, контекст, объём, бюджет, шаг количества. Возвращает: Calc. Ошибки: текст (отказ расчёта, бюджет).
/// Решение 266: тем же расчётом считается объём кандидата в быстром пересчёте предпроверки (precheck_fast.rs).
pub(super) async fn final_calc(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    budget: Decimal,
    step: Decimal,
) -> Result<Calc, String> {
    final_standard(core, cx, qty, budget, step, None).await
}

pub(super) async fn final_standard(
    core: &Core,
    cx: &DirCtx,
    qty: Decimal,
    budget: Decimal,
    step: Decimal,
    depth: Option<&mut super::final_depth::Observation>,
) -> Result<Calc, String> {
    // Standard trade budgets are in physical USDT. Mixed settlement accounts
    // for its conversion principal separately.
    let cash=(cx.usdt.ticker=="USDT" && cx.quote.first()==Some(&cx.usdt.ticker)).then_some(cx.usdt.contract).flatten();
    crate::gateway::quote_stage::with_buy_budget(&cx.dir.buy_on,cash,budget,
        final_budgeted(core,cx,qty,budget,step,depth)).await
}
async fn final_budgeted(
    core:&Core,cx:&DirCtx,qty:Decimal,budget:Decimal,step:Decimal,
    mut depth:Option<&mut super::final_depth::Observation>,
)->Result<Calc,String>{
    use super::final_depth::evaluate;
    // Решение 111: исполнитель (KyberSwap) даёт маршрут не на весь объём — пересчёт на то, что он реально исполнит
    // (вниз до шага), а не отказ. Живой случай 02.11: RVV — 359317.97 из 360732.6 (99.6%), прибыль почти та же,
    // а кандидат отброшен. Пороги прибыли проверяются как обычно — уже на меньшем объёме.
    let (c, qty) = match evaluate(core, cx, qty, depth.as_deref_mut()).await {
        Ok(c) => (c, qty),
        Err(super::size::Fail::TooLarge(t)) => {
            let fill = stage_detail(
                || "исполнимый объём у исполнителя".into(),
                final_fillable(core, cx, qty),
            )
            .await
            .map_err(|e| {
                if let Some(o) = depth.as_deref_mut() {
                    o.clear();
                }
                format!("{t}; {e}")
            })?;
            let q = if step > Decimal::ZERO {
                super::size_unit_grid::floor_buy(cx, fill, step)?
            } else {
                fill
            };
            if q <= Decimal::ZERO || q >= qty {
                if fill != qty
                    && let Some(o) = depth.as_deref_mut()
                {
                    o.clear();
                }
                return Err(t);
            }
            (
                evaluate(core, cx, q, depth.as_deref_mut())
                    .await
                    .map_err(|f| f.to_string())?,
                q,
            )
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
    let q = super::size_unit_grid::floor_buy(cx, qty * budget / c.spend_max, step)?;
    if q <= Decimal::ZERO || q >= qty {
        return Err(over(&c));
    }
    let c2 = evaluate(core, cx, q, depth)
        .await
        .map_err(|f| f.to_string())?;
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
    if !rd(&core.inner.kinds).iter().any(|(v, k)| {
        (v == &d.buy_on || v == &d.sell_on)
            && *k == crate::gateway::contract::VenueKind::Dex
            && crate::gateway::quote_stage::kyber_final(settings, v)
    }) {
        return None;
    }
    core.inner
        .deps
        .settings
        .decimal("kyber.final_gate_usd", At::default())
        .ok()
        .filter(|g| !g.is_zero())
}

/// Есть ли в направлении DEX, чья финальная котировка — у внешнего исполнителя (exec.via_kyber = да; признак фильтра
/// final_gate). Решение 266: при нём быстрый пересчёт объёма кандидата не применяется (precheck_fast.rs). Ошибок нет.
pub(super) fn final_external(core: &Core, d: &Direction) -> bool {
    let kinds = core.inner.deps.settings.venue_kinds();
    [&d.buy_on, &d.sell_on]
        .into_iter()
        .any(|v| kinds.has_dex(&v.0) && external_at(core, v))
}

/// Для RFQ внешний вызов возможен только при уже полученных действующих уровнях.
pub(super) fn external_at(core: &Core, v: &VenueId) -> bool {
    crate::gateway::quote_stage::kyber_final(&core.inner.deps.settings, v)
        || rd(&core.inner.market).has_source(v, chrono::Utc::now().timestamp_millis())
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
    let mut exact = String::new();
    if !c.max_exact {
        exact.push_str("; граница участка не уточнена до шага");
    }
    if c.capped {
        exact.push_str("; поиск ограничен, выбран лучший из посчитанных объёмов");
    }
    format!(
        "подбор выбрал {} {ticker}, прибыль {} USDT при бюджете {} USDT. \
         На границе участка поиска {} {ticker}, прибыль {} USDT. \
         Нижняя найденная точка {} {ticker}. Общий максимум объёма не установлен. \
         Посчитано объёмов {}, пиков {}{exact}",
        c.qty.normalize(),
        r(c.profit),
        r(c.budget),
        c.max_qty.normalize(),
        r(c.at_budget_profit),
        c.min_qty.normalize(),
        c.evals,
        c.peaks
    )
}
