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
//! Кто вызывает: core/precheck_sell.rs (сторона продажи — DEX).
//! От чего зависит: precheck_sell.rs (запрос продажи), dex_account.rs (вид на кошелёк направления), costs_types.rs
//! (Calc, CostLine), deps.rs (журнал, шлюз).
//! Настройки: money.min_profit_usd, money.min_profit_pct (+ через адаптер — exec.dex_slippage_pct и др.).

use rust_decimal::Decimal;

use crate::core::types::{CostKind, DexPresell, Direction};
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;

use super::costs::Calc;
use super::engine::Core;

/// Что делает: симуляция продажи DEX и пересчёт плана по её выходу. Принимает: ядро, направление, свежий расчёт.
/// Возвращает: расчёт с выручкой и прибылью по симуляции (или прежний — симуляция неприменима). Ошибки: текст «продажа
/// на B не пройдёт / невыгодна по симуляции: …».
pub async fn check(core: &Core, d: &Direction, plan: Calc) -> Result<Calc, String> {
    let b = &d.sell_on;
    let req = super::precheck_sell::request(core, d, &plan)?;
    let tax = model_tax_pct(&plan);
    let gw = super::dex_account::venue_dir(&core.inner.deps, d, b).await?;
    let sim = match gw.dex_presell(&req).await {
        Ok(s) => s,
        Err(e) if e.code == ErrorCode::NotApplicable => {
            log(
                core,
                d,
                format!("симуляция продажи до покупки не выполнена: {}", e.message),
            )
            .await;
            return Ok(plan);
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
    let min_usd = s
        .decimal("money.min_profit_usd", At::default())
        .map_err(|e| e.message)?;
    let min_pct = s
        .decimal("money.min_profit_pct", At::default())
        .map_err(|e| e.message)?;
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
    Ok(c)
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
    let pct = if plan.invested.is_zero() {
        Decimal::ZERO
    } else {
        profit / plan.invested * Decimal::ONE_HUNDRED
    };
    if profit < min_usd {
        return Err(format!(
            "невыгодна по симуляции продажи: прибыль {} USDT ниже порога {min_usd}",
            profit.round_dp(4)
        ));
    }
    if pct < min_pct {
        return Err(format!(
            "невыгодна по симуляции продажи: прибыль {}% ниже порога {min_pct}%",
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
         минимум по долям {}, модельный налог продажи {}%, блок {}, газ обмена {}, монеты в симуляции: {}",
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
        s.funding.as_deref().unwrap_or("—")
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
