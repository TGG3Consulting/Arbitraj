//! Файл: core/precheck_dex_sell.rs
//! Что делает: предпроверка стороны ПРОДАЖИ DEX до покупки (решение 234; Тигран 08.11: «посчитать всё и понять, выгодна
//! ли сделка, потом идти. Есть налог — какой угодно, или другие затраты — учитываем»). Живой случай ASS 08.11
//! (mexc → bsc_dex, сделки 20261108113609086ASS и 20261108175020857ASS): план с налогом продажи 3.166 % из пробы
//! кошельком «пробы» на ~5 USDT, а продажа кошельком сделки на сумму сделки каждый раз откатывалась «minOut» — монету
//! купили и вывели, продать не смогли. Теперь до покупки адаптер DEX симулирует настоящую продажу (контракт
//! dex_presell: путь исполнения, кошелёк сделки по маппингу, ожидаемый приход Calc::sell_qty, минимум выхода от той же
//! худшей цены, что у исполнения), и расчёт пересчитывается по РЕАЛЬНОМУ выходу симуляции: выручка = выход, прибыль =
//! прибыль плана + (выход − выручка плана), пороги money.min_profit_usd и money.min_profit_pct — те же, что у расчёта
//! (profit.rs). Откат, отказ сборки или прибыль ниже порога — сделки нет, причина (с выходом, ожиданием, минимумами и
//! модельным налогом продажи) — в журнал отказов. Адаптер симуляцию не умеет или нативную BNB без суммы на кошельке не
//! симулировать (NotApplicable) — запись в журнал и проверка как раньше (по котировке).
//! Решение 380 (задача t166; живой бот 17.11, 06:20–09:52 UTC: 350 отказов «симуляция продажи до покупки не прошла: у
//! кошелька 0 единиц входа … пула-источника … нет» — AVA, FLOKI, NXT, YCT, MIRA): адаптер не нашёл, чем пополнить
//! кошелёк на весь объём, — сделка не теряется и не идёт вслепую: план пересчитывается на объём продажи не больше
//! проверяемого (precheck_dex_cut.rs) и симулируется заново на точном новом объёме.
//! Решение 382: продажа собрана по квотеру (DexPresell.by_quoter, решение 177) — отказ «невыгодна по квотеру: прибыль …
//! ниже порога», строка расхождений — в текст.
//! Задача t179 (решение 390): при идущих сделках монеты с продажей в тот же DEX выход симуляции план не поднимает
//! (план посчитан после их монет — ride_quote.rs), только понижает.
//! Кто вызывает: core/precheck_sell.rs (сторона продажи — DEX).
//! От чего зависит: precheck_dex_cut.rs (пересчёт на проверяемый объём), precheck_sell.rs (запрос продажи), dex_account.rs (вид на кошелёк направления), costs_types.rs
//! (Calc, CostLine), deps.rs (журнал, шлюз).
//! Настройки: money.min_profit_usd, money.min_profit_pct (решение 242: площадка + тип направления — venue_dir.rs) (+ через адаптер — exec.dex_slippage_pct и др.).

use rust_decimal::Decimal;

use crate::core::types::{CostKind, DexPresell, Direction, PresellOutcome, PresellShort};
use crate::gateway::errors::ErrorCode;

use super::costs::Calc;
use super::engine::Core;

/// Что делает: симуляция продажи DEX и пересчёт плана по её выходу. Решение 380: пополнить кошелёк в симуляции на весь
/// объём нечем (PresellOutcome::Short) — план пересчитывается на объём продажи не больше проверяемого
/// (precheck_dex_cut.rs) и симулируется снова на ТОЧНОМ новом объёме; второй раз мало — отказ (повторного пересчёта нет).
/// Принимает: ядро, направление, свежий расчёт. Возвращает: расчёт с выручкой и прибылью по симуляции (или прежний —
/// симуляция неприменима). Ошибки: текст «продажа на B не пройдёт / невыгодна по симуляции: …».
pub async fn check(core: &Core, d: &Direction, plan: Calc) -> Result<Calc, String> {
    // Будущие в куче: Calc большой, а будущее предпроверки чувствительно к стеку (tests/core_dry_mode.rs).
    let (plan, short) = match Box::pin(once(core, d, plan)).await? {
        Once::Done(c) => return Ok(c),
        Once::Short(plan, short) => (plan, short),
    };
    let b = &d.sell_on;
    // Текст — как до решения 380 (код NoRoute и модельный налог продажи).
    let tax = model_tax_pct(&plan).round_dp(4).normalize();
    let fail = |why: &str| {
        format!(
            "продажа на {} не пройдёт: {why} (NoRoute); модельный налог продажи {tax}%",
            b.0
        )
    };
    if short.fundable <= Decimal::ZERO {
        return Err(fail(&short.why));
    }
    // Будущее подбора большое — в куче (стек будущего предпроверки, tests/core_dry_mode.rs).
    let cut = Box::pin(super::precheck_dex_cut::replan(core, d, &plan, &short))
        .await
        .map_err(|e| fail(&format!("{}; {e}", short.why)))?;
    let cut_qty = cut.sell_qty;
    let again = Box::pin(once(core, d, cut)).await.map_err(|e| {
        format!(
            "{e}; объём урезан до проверяемого симуляцией (решение 380): продажа {} монет вместо {}",
            cut_qty.normalize(),
            plan.sell_qty.normalize()
        )
    })?;
    match again {
        Once::Done(c) => {
            let text = super::precheck_dex_cut::note(&plan, &short, cut_qty);
            log(core, d, text).await;
            Ok(c)
        }
        Once::Short(_, again) => Err(fail(&format!(
            "{}; после пересчёта на {} монет (проверяемо {}) пополнить снова нечем: {}",
            short.why,
            cut_qty.normalize(),
            short.fundable.normalize(),
            again.why
        ))),
    }
}

/// Итог одной симуляции: пересчитанный расчёт или «пополнить нечем» (план как был и что сказал адаптер).
enum Once {
    Done(Calc),
    Short(Calc, PresellShort),
}

/// Одна симуляция продажи на объёме плана и пересчёт по её выходу (как до решения 380). Принимает: ядро,
/// направление, расчёт. Возвращает: Once. Ошибки: текст «продажа на B не пройдёт / невыгодна по симуляции: …».
async fn once(core: &Core, d: &Direction, plan: Calc) -> Result<Once, String> {
    let b = &d.sell_on;
    let req = super::precheck_sell::request(core, d, &plan)?;
    let tax = model_tax_pct(&plan);
    let gw = super::dex_account::venue_dir(&core.inner.deps, d, b).await?;
    let mut sim = match gw.dex_presell(&req).await {
        Ok(PresellOutcome::Done(s)) => s,
        Ok(PresellOutcome::Short(s)) => return Ok(Once::Short(plan, s)),
        Err(e) if e.code == ErrorCode::NotApplicable => {
            log(
                core,
                d,
                format!("симуляция продажи до покупки не выполнена: {}", e.message),
            )
            .await;
            return Ok(Once::Done(plan));
        }
        Err(e) => {
            return Err(format!(
                "продажа на {} не пройдёт: {} ({:?}); модельный налог продажи {}%",
                b.0,
                e.message,
                e.code,
                tax.round_dp(4).normalize()
            ));
        }
    };
    let s = &core.inner.deps.settings;
    // Решение 242: пороги — как у расчёта (площадка + тип направления, иначе вся система).
    let (min_usd, min_pct) = super::venue_dir::thresholds(s, super::venue_dir::of(core, d))?;
    // Задача t179 (решение 390): идут сделки монеты, чьи монеты будут проданы в тот же пул раньше нас, — симуляция
    // (на пуле до них) может план только понизить: выход выше выручки плана (план уже посчитан ПОСЛЕ их монет,
    // ride_quote.rs) не поднимает прибыль.
    if !super::ride_quote::idle(core, d)
        && super::ride_quote::sell_ahead(core, d) > Decimal::ZERO
        && sim.out > plan.proceeds
    {
        sim.out = plan.proceeds;
    }
    let text = summary(&plan, &sim, tax);
    let c = apply(plan, &sim, min_usd, min_pct).map_err(|e| format!("{e}; {text}"))?;
    log(
        core,
        d,
        format!(
            "симуляция продажи до покупки: {text}; прибыль {} USDT",
            c.profit_usd.round_dp(4)
        ),
    )
    .await;
    Ok(Once::Done(c))
}

/// Что делает: пересчёт плана по выходу симуляции. Выручка = выход симуляции (после реального налога), средняя продажи =
/// выход / количество продажи, прибыль = прибыль плана + (выход − выручка плана), % — от вложенного; пороги прибыли —
/// как в profit.rs. Принимает: расчёт, итог симуляции, порог USDT, порог %. Возвращает: пересчитанный расчёт.
/// Ошибки: текст — прибыль по симуляции ниже порога.
pub fn apply(
    mut plan: Calc,
    sim: &DexPresell,
    min_usd: Decimal,
    min_pct: Decimal,
) -> Result<Calc, String> {
    let profit = plan.profit_usd + (sim.out - plan.proceeds);
    // Решение 382: продажа собрана по квотеру (решение 177) — отказ называет это прямо.
    let what = if sim.by_quoter.is_some() {
        "по квотеру"
    } else {
        "по симуляции продажи"
    };
    let pct = if plan.invested.is_zero() {
        Decimal::ZERO
    } else {
        profit / plan.invested * Decimal::ONE_HUNDRED
    };
    if profit < min_usd {
        return Err(format!(
            "невыгодна {what}: прибыль {} USDT ниже порога {min_usd}",
            profit.round_dp(4)
        ));
    }
    if pct < min_pct {
        return Err(format!(
            "невыгодна {what}: прибыль {}% ниже порога {min_pct}%",
            pct.round_dp(4)
        ));
    }
    if !plan.sell_qty.is_zero() {
        plan.sell_avg = sim.out / plan.sell_qty;
    }
    plan.proceeds = sim.out;
    plan.profit_usd = profit;
    plan.profit_pct = pct;
    Ok(plan)
}

/// Модельный налог продажи плана, %: строки SellTax стороны B (в монете) к количеству продажи. Ошибок нет (нет
/// строк или количества — 0).
pub fn model_tax_pct(plan: &Calc) -> Decimal {
    let coins: Decimal = plan
        .costs
        .iter()
        .filter(|l| l.kind == CostKind::SellTax && l.stage == "B")
        .map(|l| l.amount)
        .sum();
    if plan.sell_qty.is_zero() {
        Decimal::ZERO
    } else {
        coins / plan.sell_qty * Decimal::ONE_HUNDRED
    }
}

/// Строка для журнала: кошелёк, форма, выход против плана, ожидаемое, минимумы, модельный налог, пополнение. Ошибок нет.
fn summary(plan: &Calc, s: &DexPresell, tax: Decimal) -> String {
    let mins: Vec<String> = s
        .min_legs
        .iter()
        .map(|m| m.round_dp(6).normalize().to_string())
        .collect();
    format!(
        "кошелёк {}, {}, продано {} монет, выход {} USDT против выручки плана {} (разница {}), ожидалось {} USDT, \
         минимум по долям {}, модельный налог продажи {}%, блок {}, газ обмена {}, монеты в симуляции: {}{}",
        s.wallet,
        s.form,
        s.coins.normalize(),
        s.out.round_dp(6),
        plan.proceeds.round_dp(6),
        (s.out - plan.proceeds).round_dp(6),
        s.expected.round_dp(6),
        mins.join("; "),
        tax.round_dp(4).normalize(),
        s.block,
        s.gas_swap,
        s.funding.as_deref().unwrap_or("—"),
        s.by_quoter
            .as_ref()
            .map_or(String::new(), |q| format!("; продажа по квотеру: {q}"))
    )
}

/// Запись в журнал (core.precheck). Ошибок нет.
async fn log(core: &Core, d: &Direction, text: String) {
    let msg = format!(
        "{} {} → {}: {text}",
        d.asset.ticker, d.buy_on.0, d.sell_on.0
    );
    core.inner
        .deps
        .log(
            crate::storage::journal::Level::Info,
            "core.precheck",
            Some(&d.sell_on.0),
            msg,
            None,
        )
        .await;
}
