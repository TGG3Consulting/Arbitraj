//! Файл: core/size.rs
//! Что делает: типы подбора объёма сделки (решение 66): почему объём не подходит (меньше минимума, больше возможного,
//! расчёт невозможен), расчёт в точке, кривая «прибыль от объёма», параметры поиска и итог с объяснением.
//! Сам поиск — size_search.rs (границы — size_bounds.rs), кривая по costs.rs — size_curve.rs.
//! Кто вызывает: core/profit.rs, core/costs.rs (Fail), size_search.rs, size_bounds.rs, size_curve.rs, тесты.
//! От чего зависит: rust_decimal, async_trait, serde.
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
    /// Расчёт невозможен при любом объёме (нет данных, не задана настройка, сбой шлюза) — сделки нет.
    Stop(String),
}

impl Fail {
    /// Текст причины (для журнала и «Кандидатов»). Ошибок нет.
    pub fn text(&self) -> &str {
        match self {
            Fail::TooSmall(t) | Fail::TooLarge(t) | Fail::Stop(t) => t,
        }
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
}

/// Итог поиска: лучшая точка и сводка.
#[derive(Debug, Clone)]
pub struct Found<T> {
    pub best: Point<T>,
    pub choice: SizeChoice,
}
