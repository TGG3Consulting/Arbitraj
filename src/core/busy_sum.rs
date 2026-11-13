//! Файл: core/busy_sum.rs
//! Что делает: решение 267 — сводка теневых расчётов направления за время, пока монета занята сделкой (замок «одна
//! сделка на монету», решение 118 Я-2): чистая логика без диска, сети и настроек. Только агрегаты (память не растёт
//! с числом расчётов): сколько расчётов, сколько прошло пороги, сколько отброшено без расчёта (места расчёта заняты
//! настоящими кандидатами), первый и последний момент, расчёт с наибольшей прибылью среди прошедших пороги (прибыль,
//! вложение, объём, время, бюджет расчёта и составляющие лимита — чем ограничен), наибольшее вложение среди прошедших,
//! текст последнего отказа, наша сделка для сравнения (id, вложено, план). Текст одной строки журнала core.busy_calc.
//! Кто вызывает: core/busy_calc.rs (итог расчёта), core/busy_note.rs (учёт и журнал), core/window_info.rs (поле сводки
//! окна busy), core/window.rs (note_busy), тесты.
//! От чего зависит: core/types (Direction), limits_parts.rs (Parts), chrono, rust_decimal, serde.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::Direction;

use super::limits_parts::Parts;

/// Теневой расчёт, прошедший пороги (profit::decide → Accept), — числа для сводки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusyBest {
    /// Прибыль после всех затрат, USDT (Calc.profit_usd).
    pub profit_usd: Decimal,
    /// Вложение, USDT (Calc.invested: объём × средняя цена покупки).
    pub invested: Decimal,
    /// Объём покупки на A, монет (Calc.qty).
    pub qty: Decimal,
    /// Момент расчёта (Calc.at).
    pub at: DateTime<Utc>,
    /// Бюджет расчёта, USDT (SizeChoice.budget) — действующие лимиты с учётом нашей открытой сделки по монете.
    pub budget: Option<Decimal>,
    /// Составляющие лимита сразу после расчёта (limits_parts.rs) — какая ограничила бюджет. None — не прочитаны.
    pub limits: Option<Parts>,
}

/// Наша сделка, которой занята монета, — для сравнения (из учёта ядра, без запросов).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusyDeal {
    pub id: String,
    /// Направление сделки (замок — по монете: сделка может идти по другому направлению той же монеты).
    pub direction: Direction,
    /// Вложено по факту на момент записи, USDT (Deal.invested; до покупки — 0).
    pub invested: Decimal,
    /// Вложение плана сделки, USDT (поле invested расчёта в Deal.plan). None — в плане нет.
    pub plan_invested: Option<Decimal>,
    /// Плановая прибыль, USDT (Deal.plan_profit_usd).
    pub plan_profit_usd: Decimal,
}

/// Итог одного теневого расчёта.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Прошёл пороги.
    Accept(Box<BusyBest>),
    /// Не прошёл — причина (текст отказа profit::decide или срок расчёта).
    Reject(String),
    /// Отброшен без расчёта: места расчёта заняты настоящими кандидатами (или снят, уступая место настоящему).
    Dropped,
}

/// Сводка теневых расчётов направления за время занятости монеты.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusySum {
    pub direction: Direction,
    /// Сколько расчётов дали итог (прошёл / не прошёл).
    pub calcs: u64,
    /// Сколько из них прошло пороги (Accept).
    pub accepted: u64,
    /// Сколько отброшено без расчёта (места расчёта заняты настоящими кандидатами).
    pub dropped: u64,
    pub first_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    /// Расчёт с наибольшей прибылью среди прошедших пороги.
    pub best: Option<BusyBest>,
    /// Наибольшее вложение среди прошедших пороги, USDT.
    pub max_invested: Option<Decimal>,
    /// Текст последнего отказа (чем не прошёл — например «нет бюджета: …»).
    pub last_reject: Option<String>,
    /// Наша сделка по монете, если она видна в учёте (замок без открытой сделки — None).
    pub deal: Option<BusyDeal>,
}

impl BusySum {
    /// Пустая сводка направления. Принимает: направление, время. Ошибок нет.
    pub fn new(direction: Direction, now: DateTime<Utc>) -> Self {
        BusySum {
            direction,
            calcs: 0,
            accepted: 0,
            dropped: 0,
            first_at: now,
            last_at: now,
            best: None,
            max_invested: None,
            last_reject: None,
            deal: None,
        }
    }

    /// Что делает: учитывает итог теневого расчёта. Максимум — наибольшая прибыль среди прошедших пороги (при равной —
    /// первый). Принимает: итог, время. Ошибок нет.
    pub fn note(&mut self, o: Outcome, now: DateTime<Utc>) {
        self.last_at = now;
        match o {
            Outcome::Dropped => self.dropped += 1,
            Outcome::Reject(why) => {
                self.calcs += 1;
                self.last_reject = Some(why);
            }
            Outcome::Accept(b) => {
                self.calcs += 1;
                self.accepted += 1;
                let m = self.max_invested;
                self.max_invested = Some(m.map_or(b.invested, |x| x.max(b.invested)));
                if self
                    .best
                    .as_ref()
                    .is_none_or(|x| b.profit_usd > x.profit_usd)
                {
                    self.best = Some(*b);
                }
            }
        }
    }

    /// Что делает: одна строка журнала core.busy_calc (решение 267, п. 4). Принимает: сводку. Ошибок нет.
    pub fn text(&self) -> String {
        let d = &self.direction;
        let r = |v: Decimal| v.round_dp(4).normalize();
        let deal = match &self.deal {
            Some(x) => {
                let other = if &x.direction == d {
                    String::new()
                } else {
                    format!(
                        " по направлению {} → {}",
                        x.direction.buy_on.0, x.direction.sell_on.0
                    )
                };
                let plan = x
                    .plan_invested
                    .map(|p| format!(", вложение плана {} USDT", r(p)))
                    .unwrap_or_default();
                format!(
                    "сделкой {}{other} (вложено {} USDT{plan}, план {})",
                    x.id,
                    r(x.invested),
                    r(x.plan_profit_usd)
                )
            }
            None => "(открытой сделки по монете в учёте не видно)".to_string(),
        };
        let dropped = match self.dropped {
            0 => String::new(),
            n => format!(", отброшено без расчёта (места расчёта заняты) {n}"),
        };
        let tail = match (&self.best, self.max_invested) {
            (Some(b), Some(w)) => {
                let budget = match (b.budget, &b.limits) {
                    (Some(v), Some(p)) => format!(
                        ", бюджет расчёта {} USDT (ограничил лимит {})",
                        r(v),
                        binding(p)
                    ),
                    (Some(v), None) => format!(", бюджет расчёта {} USDT", r(v)),
                    _ => String::new(),
                };
                format!(
                    "максимум прибыли {} USDT при вложении {} USDT{} в {} UTC{budget}; наибольшее прибыльное \
                     вложение {} USDT",
                    r(b.profit_usd),
                    r(b.invested),
                    self.ratio(b.invested),
                    b.at.format("%H:%M:%S"),
                    r(w)
                )
            }
            _ => match &self.last_reject {
                Some(why) => format!("прибыльных расчётов не было; последний отказ: {why}"),
                None => "прибыльных расчётов не было".to_string(),
            },
        };
        format!(
            "{} {} → {}: пока монета была занята {deal}: теневых расчётов {}, прошло пороги {}{dropped}; {tail}",
            d.asset.ticker, d.buy_on.0, d.sell_on.0, self.calcs, self.accepted
        )
    }

    /// Во сколько раз вложение теневого максимума больше или меньше вложения сделки (факт; до покупки — план).
    /// Принимает: вложение максимума. Возвращает: текст в скобках с пробелом впереди; пусто — сравнить не с чем.
    fn ratio(&self, v: Decimal) -> String {
        let base = self.deal.as_ref().and_then(|x| {
            Some(x.invested)
                .filter(|i| *i > Decimal::ZERO)
                .or(x.plan_invested.filter(|i| *i > Decimal::ZERO))
        });
        match base {
            Some(b) if v > b => {
                format!(" (в {} раз больше сделки)", (v / b).round_dp(2).normalize())
            }
            Some(b) if v < b && v > Decimal::ZERO => {
                format!(" (в {} раз меньше сделки)", (b / v).round_dp(2).normalize())
            }
            Some(_) if v > Decimal::ZERO => " (как у сделки)".to_string(),
            _ => String::new(),
        }
    }
}

/// Какая составляющая ограничила бюджет — по-русски (limits_parts.rs binding). Ошибок нет.
fn binding(p: &Parts) -> &'static str {
    match p.binding() {
        "per_trade" => "на сделку",
        "per_coin" => "на монету",
        "in_transit" => "в пути",
        _ => "сухого режима",
    }
}
