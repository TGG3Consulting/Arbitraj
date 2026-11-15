//! Файл: core/precheck_fast.rs
//! Что делает: решение 266 (Тигран 13.11.2026) — быстрая предпроверка, часть 1: вместо повторного полного подбора
//! объёма (решение 66; живые замеры 13.11 — 579–2825 мс, 91–136 расчётов) пересчёт предпроверки считает на свежих
//! данных ОДИН объём — объём кандидата (Calc::qty расчёта, прошедшего пороги) — тем же финальным расчётом
//! profit.rs final_calc (все затраты, финальная котировка, бюджет). Прибыль не ниже порогов money.min_profit_usd и
//! money.min_profit_pct — это план сделки; любой другой исход (ошибка расчёта, ниже порога) — без отказа выполняется
//! прежний полный подбор (profit.rs decide_capped, без изменений). Все проверки до подбора (делистинг, закрытые входы,
//! свежесть налогов, контекст, пороги, бюджет, цена покупки, параметры) — общие, ни одна не пропускается.
//! Финальная котировка направления у внешнего исполнителя (exec.via_kyber = да на DEX направления — тот же признак,
//! что у фильтра kyber.final_gate_usd) — быстрый путь не применяется: лишнего запроса к KyberSwap нет, фильтр не
//! обходится. Здесь же — чтение всех четырёх включателей решения 266 (один раз на сделку) и строка хронологии
//! «План: …». Часть 4 (precheck.balance_first, precheck_balance.rs): расчёт идёт с пределом min(бюджет, свободно); если
//! списание кандидата больше этого предела — быстрый путь не применяется, сразу полный подбор под предел.
//! Кто вызывает: core/executor.rs (read), core/precheck.rs (Want, строка), core/profit.rs (recalc_boxed из
//! decide_capped).
//! От чего зависит: core/profit.rs (final_calc, final_external), core/calc_timing.rs (этап замера), core/size.rs.
//! Настройки: precheck.fast_recalc, precheck.parallel, precheck.networks_memory, precheck.balance_first (да/нет, вся
//! система; не заданы или «нет» — как до решения 266).

use std::future::Future;
use std::pin::Pin;

use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::settings::store::Store;

use super::calc_timing::stage;
use super::costs::Calc;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::precheck_balance::{BALANCE, Cand};
use super::precheck_nets::MEMORY;
use super::size::SizeChoice;

/// Включатель части 1 (пересчёт только объёма кандидата).
pub const FAST: &str = "precheck.fast_recalc";
/// Включатель части 2 (независимые запросы предпроверки одновременно, precheck_par.rs).
pub const PARALLEL: &str = "precheck.parallel";
/// Имя этапа замера (решения 129, 265): отдельное от «подбор объёма» и «финальная котировка».
pub const STAGE: &str = "пересчёт объёма кандидата";

/// Режим предпроверки сделки (решение 266). По умолчанию — всё выключено (как до решения 266).
#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// Объём кандидата для пересчёта (precheck.fast_recalc = да); None — полный подбор, как раньше.
    pub qty: Option<Decimal>,
    /// precheck.parallel = да — запросы сторон и удержаний вывода одновременно.
    pub parallel: bool,
    /// precheck.networks_memory = да — справочник сетей из памяти, пока она свежая (precheck_nets.rs).
    pub networks_memory: bool,
    /// precheck.balance_first = да — числа кандидата: баланс A читается перед расчётом (precheck_balance.rs).
    pub balance: Option<Cand>,
}

/// Читает включатели решения 266 — один раз на сделку (executor.rs accepted, как log.step_timing). Не задана или не
/// читается — выключено (не ошибка, журнала нет). Принимает: настройки, расчёт кандидата. Ошибок нет.
pub fn read(s: &Store, cand: &Calc) -> Opts {
    let flag = |name: &str| s.flag(name, At::default()).unwrap_or(false);
    Opts {
        qty: flag(FAST).then_some(cand.qty),
        parallel: flag(PARALLEL),
        networks_memory: flag(MEMORY),
        balance: flag(BALANCE).then(|| Cand::of(cand)),
    }
}

/// Каким путём получен план пересчёта.
#[derive(Debug, Clone, PartialEq)]
pub enum Path {
    /// Объём кандидата пересчитан и прошёл пороги — подбора не было.
    Used,
    /// Пересчёт объёма кандидата не прошёл (причина) — дальше полный подбор.
    Full(String),
    /// Быстрый путь не применяется (причина) — сразу полный подбор.
    Off(String),
}

/// Запрос быстрого пути и его исход (заполняет recalc). path None — пересчёт до быстрого пути не дошёл (отказ раньше).
#[derive(Debug, Clone)]
pub struct Want {
    pub qty: Decimal,
    pub path: Option<Path>,
    /// Часть 4 (баланс A получен): списание кандидата по худшей цене — больше предела расчёта (min(бюджет,
    /// свободно)) — быстрый путь не применяется, сразу полный подбор под предел. None — без этой проверки.
    pub spend: Option<Decimal>,
}

impl Want {
    /// Запрос на объём кандидата. Ошибок нет.
    pub fn new(qty: Decimal) -> Self {
        Want {
            qty,
            path: None,
            spend: None,
        }
    }

    /// Строка хронологии сделки и журнала core.precheck: каким путём получен план. None — до быстрого пути не дошли.
    /// Ошибок нет.
    pub fn line(&self) -> Option<String> {
        Some(match self.path.as_ref()? {
            Path::Used => {
                "План: объём кандидата пересчитан на свежих данных (без повторного подбора)".into()
            }
            Path::Full(why) => {
                format!(
                    "План: пересчёт объёма кандидата не прошёл ({why}) — выполнен полный подбор"
                )
            }
            Path::Off(why) => {
                format!(
                    "План: пересчёт объёма кандидата не применялся ({why}) — выполнен полный подбор"
                )
            }
        })
    }
}

/// Что делает: recalc с будущим в куче. Будущее финального расчёта большое: собранное прямо в profit.rs decide_capped,
/// оно раздувает кадр стека расчёта каждого кандидата (в отладочной сборке — до переполнения стека); здесь оно
/// собирается в своём кадре, только когда объём задан. Принимает, возвращает: как recalc.
#[inline(never)]
pub(super) fn recalc_boxed<'a>(
    core: &'a Core,
    cx: &'a DirCtx,
    w: &'a mut Want,
    budget: Decimal,
    step: Decimal,
    min: (Decimal, Decimal),
) -> Pin<Box<dyn Future<Output = Option<Calc>> + Send + 'a>> {
    Box::pin(recalc(core, cx, w, budget, step, min))
}

/// Что делает: быстрый путь пересчёта — финальный расчёт на объёме кандидата (вниз до шага количества) и те же
/// сравнения с порогами, что в конце profit.rs decide_capped. Принимает: ядро, контекст направления, запрос (исход
/// пишется в него), бюджет этого пересчёта, шаг количества, пороги (USDT, %). Возвращает: Some(расчёт с объяснением
/// выбора) — план принят; None — быстрый путь не дал плана, вызывающий выполняет полный подбор. Ошибок нет — причина
/// в `w.path`.
pub async fn recalc(
    core: &Core,
    cx: &DirCtx,
    w: &mut Want,
    budget: Decimal,
    step: Decimal,
    (min_usd, min_pct): (Decimal, Decimal),
) -> Option<Calc> {
    let d = &cx.dir;
    if super::profit::final_external(core, d) {
        w.path = Some(Path::Off(
            "финальная котировка направления — у внешнего исполнителя, exec.via_kyber".into(),
        ));
        return None;
    }
    // Часть 4: кандидату нужно больше предела (свободный баланс или бюджет) — сразу полный подбор под предел.
    if let Some(spend) = w.spend.filter(|s| *s > budget) {
        w.path = Some(Path::Off(format!(
            "списание кандидата {} больше предела списания {budget} (свободный баланс или бюджет) — объём \
             подбирается под предел",
            spend.round_dp(4)
        )));
        return None;
    }
    let qty = match floor_qty(w.qty, step) {
        Ok(q) => q,
        Err(e) => {
            w.path = Some(Path::Full(e));
            return None;
        }
    };
    let fin = super::profit::final_calc(core, cx, qty, budget, step);
    let done = match stage(|| STAGE.into(), fin).await {
        Ok(c) => accept(c, qty, budget, &d.asset.ticker, (min_usd, min_pct)),
        Err(e) => Err(e),
    };
    match done {
        Ok(c) => {
            w.path = Some(Path::Used);
            Some(c)
        }
        Err(e) => {
            w.path = Some(Path::Full(e));
            None
        }
    }
}

/// Объём кандидата вниз до шага количества. Принимает: объём, шаг. Возвращает: объём для финального расчёта (> 0).
/// Ошибки (быстрый путь не считается — полный подбор, он сам откажет, как раньше): шаг не больше нуля; объём после
/// округления не больше нуля.
pub fn floor_qty(qty: Decimal, step: Decimal) -> Result<Decimal, String> {
    if step <= Decimal::ZERO {
        return Err(format!("шаг количества {step} не больше нуля"));
    }
    let q = (qty / step).floor() * step;
    if q <= Decimal::ZERO {
        return Err(format!(
            "объём кандидата {} вниз до шага количества {step} не больше нуля",
            qty.normalize()
        ));
    }
    Ok(q)
}

/// Решение быстрого пути по готовому финальному расчёту: те же сравнения с порогами, что в конце profit.rs
/// decide_capped (прибыль не ниже money.min_profit_usd и money.min_profit_pct); прошёл — расчёт с объяснением выбора
/// (choice). Принимает: расчёт, запрошенный объём (после шага), бюджет пересчёта, тикер, пороги (USDT, %).
/// Возвращает: план. Ошибки: текст — прибыль ниже порога (дальше — полный подбор).
pub fn accept(
    mut c: Calc,
    asked: Decimal,
    budget: Decimal,
    ticker: &str,
    (min_usd, min_pct): (Decimal, Decimal),
) -> Result<Calc, String> {
    if c.profit_usd < min_usd {
        return Err(format!(
            "прибыль {} USDT ниже порога {min_usd}",
            c.profit_usd.round_dp(4)
        ));
    }
    if c.profit_pct < min_pct {
        return Err(format!(
            "прибыль {}% ниже порога {min_pct}%",
            c.profit_pct.round_dp(4)
        ));
    }
    c.size = Some(choice(&c, asked, budget, ticker));
    Ok(c)
}

/// Объяснение выбора объёма у быстрого пути — честно: подбора не было, посчитан один объём (сколько раз final_calc
/// пересчитывал внутри — не известно, записан один расчёт), границы — сам объём, «на бюджете» — он же. Исполнение читает
/// отсюда только budget (executor.rs, uncapped_entry.rs), люди — why. Принимает: расчёт, запрошенный объём (после шага),
/// бюджет пересчёта, тикер. Ошибок нет.
pub fn choice(c: &Calc, asked: Decimal, budget: Decimal, ticker: &str) -> SizeChoice {
    let mut why = format!(
        "объём кандидата {} {ticker} пересчитан на свежих данных: прибыль {}; повторный подбор объёма не выполнялся \
         ({FAST})",
        asked.normalize(),
        c.profit_usd.round_dp(4).normalize()
    );
    // Как у полного пути после финальной котировки: объём сделки — фактический.
    if c.qty != asked {
        why = format!(
            "{why}; по финальной котировке объём ужат (бюджет или глубина исполнителя): {} {ticker}",
            c.qty.normalize()
        );
    }
    SizeChoice {
        qty: c.qty,
        profit: c.profit_usd,
        min_qty: c.qty,
        max_qty: c.qty,
        budget,
        at_budget_profit: c.profit_usd,
        max_exact: false,
        evals: 1,
        peaks: 0,
        capped: false,
        why,
    }
}
