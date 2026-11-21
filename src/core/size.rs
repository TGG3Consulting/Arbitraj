//! Файл: core/size.rs
//! Что делает: типы подбора объёма сделки (решение 66): почему объём не подходит (меньше минимума, больше возможного,
//! расчёт невозможен), расчёт в точке, кривая «прибыль от объёма», параметры поиска и итог с объяснением.
//! Сам поиск — size_search.rs (границы — size_bounds.rs), кривая по costs.rs — size_curve.rs.
//! Кто вызывает: core/profit.rs, core/costs.rs (Fail), size_search.rs, size_bounds.rs, size_curve.rs, тесты.
//! От чего зависит: rust_decimal, async_trait, serde.
//! Решение 336: Curve::batch — пачка независимых точек (по умолчанию по одной); Params.par_points, bisect_probes.
//! Решение 337: Params.early_floor и Found.early — ранний выход подбора (size_search.rs).
//! Задача t159 (решения 342–345, ревью t159): Params.ext — части подбора без потолка прибыли (size_extra.rs); Curve::pct
//! и Curve::pct_base — процент прибыли точки и его база (порог % — ограничение поиска, size_pct.rs); Curve::breaks —
//! изломы пулов DEX-сторон маршрута точки (size_breaks.rs); Found.late — подбор остановлен мягким сроком; Found.pct_moved
//! и Found.fallback — выбор сдвинут порогом % и следующий меньший проходящий объём (один пересчёт финальной).
//! Настройки: нет (параметры поиска читает size_curve.rs: size.*).

use std::fmt;

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::Serialize;

/// Почему расчёт на объём не получился. От вида зависит, куда двигать объём.
#[derive(Debug, Clone, PartialEq)]
pub enum Fail {
    /// Объём меньше минимума площадок (вывод, депозит, сумма ордера) — больший объём может подойти.
    TooSmall(String),
    /// Объём больше возможного (глубина, максимум вывода или суммы ордера, бюджет) — меньший может подойти.
    TooLarge(String),
    /// Ревью t159 Н-4: вложение (списание по худшей цене) больше бюджета — «слишком большой» со своим видом (без
    /// разбора текста; size_memo.rs over_budget): меньший объём может подойти; при монотонном списании выше — тоже нет.
    OverBudget(String),
    /// Расчёт невозможен при любом объёме (нет данных, не задана настройка, сбой шлюза) — сделки нет.
    Stop(String),
}

impl Fail {
    /// Текст причины (для журнала и «Кандидатов»). Ошибок нет.
    pub fn text(&self) -> &str {
        match self {
            Fail::TooSmall(t) | Fail::TooLarge(t) | Fail::OverBudget(t) | Fail::Stop(t) => t,
        }
    }

    /// «Слишком большой» любого вида (TooLarge или OverBudget) — меньший объём может подойти. Ошибок нет.
    pub fn too_large(&self) -> bool {
        matches!(self, Fail::TooLarge(_) | Fail::OverBudget(_))
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

impl From<String> for Fail {
    /// Любая ошибка без вида — расчёт невозможен (Stop).
    fn from(s: String) -> Self {
        Fail::Stop(s)
    }
}

/// Расчёт на один объём.
#[derive(Debug, Clone)]
pub struct Point<T> {
    /// Сколько покупаем на A (монет).
    pub qty: Decimal,
    /// Списание по бюджету на A, USDT: наибольшее, что может уйти или замёрзнуть (у ядра — Calc.spend_max).
    pub invested: Decimal,
    /// Сколько дойдёт до B (монет) — для пересчёта изломов стакана B в объём покупки.
    pub arrive: Decimal,
    /// Прибыль после всех затрат, USDT.
    pub profit: Decimal,
    /// Полный расчёт (у ядра — costs::Calc).
    pub data: T,
}

/// Кривая «прибыль от объёма»: расчёт на объём qty.
#[async_trait]
pub trait Curve: Send {
    type Data: Clone + Send + Sync;
    /// Принимает: объём (уже кратный шагу). Возвращает: расчёт. Ошибки: Fail с видом причины.
    async fn at(&mut self, qty: Decimal) -> Result<Point<Self::Data>, Fail>;

    /// Решение 336: расчёт пачки объёмов, независимых друг от друга (size_memo.rs at_many при size.parallel_points > 1).
    /// По умолчанию — по одному, по порядку (как at). Кривая ядра считает пачку одновременно (size_curve.rs,
    /// cycle_curve.rs): котировки DEX каждой точки идут своими потоками счёта адаптера.
    /// Принимает: объёмы (кратные шагу). Возвращает: итоги в том же порядке. Ошибки: у каждого итога — как у at.
    async fn batch(&mut self, qtys: Vec<Decimal>) -> Vec<Result<Point<Self::Data>, Fail>> {
        let mut out = Vec::with_capacity(qtys.len());
        for q in qtys {
            out.push(self.at(q).await);
        }
        out
    }

    /// Решение 342: процент прибыли точки от вложенного (для порога % как ограничения поиска, size_pct.rs). По
    /// умолчанию — None (кривая процента не знает: ограничение не действует). Принимает: данные точки. Ошибок нет.
    fn pct(_data: &Self::Data) -> Option<Decimal> {
        None
    }

    /// Ревью t159 Б1: база процента — вложение, от которого он считается (у ядра Calc.invested = объём × средняя цена
    /// покупки, costs.rs; не списание по худшей цене). Нужна для запаса по порогу % в USDT: g = прибыль − порог/100 ×
    /// база (size_pct.rs). По умолчанию — None (ограничение не действует). Принимает: данные точки. Ошибок нет.
    fn pct_base(_data: &Self::Data) -> Option<Decimal> {
        None
    }

    /// Решение 344 (ревью t159 Б4): изломы кривых пулов DEX-сторон МАРШРУТА точки `data` (лучшей посчитанной): объёмы
    /// покупки (DEX на A) и объёмы продажи (DEX на B; в объём покупки их переводит ядро по приходу, как уровни bids).
    /// По умолчанию — пусто (DEX-стороны нет). Принимает: данные точки, наибольший объём (монет), сколько изломов
    /// отдать наибольшим числом на сторону. Возвращает: (объёмы покупки, объёмы продажи). Ошибок нет: сбой — пусто
    /// (изломы — только точки подбора, не данные решения).
    async fn breaks(
        &mut self,
        _data: &Self::Data,
        _max_qty: Decimal,
        _limit: u32,
    ) -> (Vec<Decimal>, Vec<Decimal>) {
        (Vec::new(), Vec::new())
    }
}

/// Параметры поиска. Все из настроек и данных площадок — значений по умолчанию нет.
#[derive(Debug, Clone)]
pub struct Params {
    /// Шаг количества (stepSize биржи), > 0. Все объёмы поиска кратны ему (округление вниз).
    pub step: Decimal,
    /// Бюджет: вложение не больше его — инвариант.
    pub budget: Decimal,
    /// Оценка наибольшего объёма (бюджет / лучшая цена) — откуда начинать поиск границы бюджета.
    pub guess: Decimal,
    /// Изломы по объёму покупки (накопленные объёмы уровней стакана A).
    pub buy_breaks: Vec<Decimal>,
    /// Изломы по приходу на B (накопленные объёмы уровней стакана B).
    pub sell_breaks: Vec<Decimal>,
    /// size.bisect_steps — наибольшее число шагов бисекции каждой границы (бюджет, минимумы).
    pub bisect_steps: u32,
    /// size.grid_points — сколько равномерных точек между границами сверх изломов.
    pub grid_points: u32,
    /// size.refine_steps — наибольшее число шагов тернарного поиска у каждого локального максимума.
    pub refine_steps: u32,
    /// size.tolerance_pct — уточнение останавливается, когда отрезок не шире этой доли наибольшего объёма (в %).
    pub tolerance_pct: Decimal,
    /// size.max_evals — общий предел расчётов на кандидата. Исчерпан на границах — отказ; дальше — лучший из
    /// посчитанных (каждый из них в бюджете).
    pub max_evals: u32,
    /// Решение 336: size.parallel_points — сколько независимых точек считать одновременно (изломы и сетка; уточнение
    /// всех пиков разом, по две точки тернарного шага на пик). 1 — по одной, как до 336.
    pub par_points: u32,
    /// Решение 336: size.bisect_probes — проб за шаг поиска границ (k-ичный поиск вместо двоичного). 1 — двоичный, как
    /// до 336.
    pub bisect_probes: u32,
    /// Решение 337: пол раннего выхода — порог прибыли минус запас size.early_exit_margin_usd (в единицах прибыли
    /// точки, USDT). Лучшая посчитанная точка после сетки (и после изломов) ниже пола — уточнения нет, Found.early.
    /// None — без раннего выхода (как до 337).
    pub early_floor: Option<Decimal>,
    /// Решение 336: повторное использование того, что не зависит от объёма (у направления с DEX — решение 363, size_curve.rs) —
    /// короб подбора с reuse (gateway/calc_share.rs). false — нет (у цикла DEX ↔ DEX — всегда, cycle_plan.rs).
    pub reuse: bool,
    /// Ревью t157 Н3: size.parallel_points задана, а cpu.threads_max нет — точки по одной, пояснение в why (profit.rs).
    pub par_blocked: bool,
    /// Задача t159: части подбора решений 342–345 (size_extra.rs). По умолчанию — пусто (как до t159).
    pub ext: super::size_extra::Extra,
}

/// Итог подбора — для журнала и «Кандидатов».
#[derive(Debug, Clone, Serialize)]
pub struct SizeChoice {
    /// Найденный объём и его прибыль.
    pub qty: Decimal,
    pub profit: Decimal,
    /// Наименьший допустимый объём (минимумы площадок) и наибольший (вложение ≤ бюджета, глубина).
    pub min_qty: Decimal,
    pub max_qty: Decimal,
    pub budget: Decimal,
    /// Прибыль на наибольшем объёме («на бюджете»).
    pub at_budget_profit: Decimal,
    /// Граница бюджета найдена точно (до шага) — иначе не хватило size.bisect_steps, взята безопасная сторона.
    pub max_exact: bool,
    /// Сколько объёмов посчитано.
    pub evals: u32,
    /// Сколько локальных максимумов уточнялось.
    pub peaks: u32,
    /// Поиск остановлен пределом size.max_evals — взят лучший из посчитанных.
    pub capped: bool,
    /// Почему этот объём лучший — человеческим текстом (заполняет profit.rs).
    pub why: String,
    /// Решение 407 (t192): объём урезан симуляцией продажи до покупки (решение 380) — настоящая причина ограничения
    /// (ставит core/precheck_dex_cut.rs mark); None — не урезался.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presell_cut: Option<String>,
}

/// Итог поиска: лучшая точка и сводка.
#[derive(Debug, Clone)]
pub struct Found<T> {
    pub best: Point<T>,
    pub choice: SizeChoice,
    /// Решение 337: подбор остановлен ранним выходом (лучшая прибыль ниже Params.early_floor) — уточнения не было.
    pub early: bool,
    /// Решение 345: подбор остановлен мягким сроком size.soft_deadline_pct — итог из посчитанных (среди них есть
    /// проходящий оба порога).
    pub late: bool,
    /// Решение 342: лучший по USDT объём не проходит порог % — итог сдвинут к объёму, проходящему его.
    pub pct_moved: bool,
    /// Ревью t159 Н2: при сдвиге порогом % — ближайший меньший посчитанный объём, проходящий оба порога (один пересчёт
    /// финальной котировкой, если на выбранном она не прошла порог %). None — нет такого или сдвига не было. В куче:
    /// Found лежит в будущем profit::decide через финальную котировку — второй расчёт по значению раздул бы его кадр
    /// (tests/core_dry_mode.rs precheck_replans_live — предел стека).
    pub fallback: Option<Box<Point<T>>>,
}
