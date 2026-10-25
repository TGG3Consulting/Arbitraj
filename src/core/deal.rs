//! Файл: core/deal.rs
//! Что делает: сделка ядра — план (расчёт кандидата) и факт по шагам: покупка → перевод → продажа,
//! застрявшая монета, итог. Сделка ведёт учёт: где сейчас монеты бота, сколько вложено, что потрачено.
//! Кто вызывает: executor.rs, transfers.rs, stuck.rs, manual.rs, ledger.rs (хранение и экран «Сделки»).
//! От чего зависит: core/types, costs.rs (план).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::types::{Direction, TradeResult, Transfer, VenueId};

/// Шаг сделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    /// Предпроверка и покупка на A.
    Buying,
    /// Вывод с A и ожидание зачисления.
    Transfer,
    /// Продажа на площадке, где сейчас монеты.
    Selling,
    /// Застрявшая монета: идёт режим решения 37.
    Stuck,
    /// Ждёт решения человека (алерт «Порог убытка»).
    WaitingHuman,
    /// Сделка закрыта: монет у бота не осталось.
    Closed,
    /// Остановлена при отклонении или сбое — монеты (если есть) остаются на площадке, нужна проверка.
    Halted,
    /// Монеты забрал человек («Беру на себя»).
    TakenOver,
}

/// Сделка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deal {
    pub id: String,
    pub direction: Direction,
    /// План — расчёт кандидата (costs::Calc в JSON), как он был в момент решения.
    pub plan: Value,
    pub plan_profit_usd: Decimal,
    pub step: Step,
    /// Где сейчас монеты бота и сколько их.
    pub at: VenueId,
    pub qty: Decimal,
    /// Вложено USDT (покупка + затраты, уже оплаченные в USDT).
    pub invested: Decimal,
    /// Получено USDT от продаж.
    pub proceeds: Decimal,
    /// Затраты в USDT вне цены (комиссии продажи, газ, вывод) — по факту, сколько известно.
    pub spent: Decimal,
    pub buy: Option<TradeResult>,
    pub transfers: Vec<Transfer>,
    pub sells: Vec<TradeResult>,
    /// Пыль (решение 66): остаток после продажи меньше шага количества или минимума ордера биржи — продать нельзя,
    /// остаётся на балансе площадки; его стоимость уже в затратах расчёта (StepRemainder). Сделку не держит.
    #[serde(default)]
    pub dust: Decimal,
    /// Сколько раз монету перегоняли (режим «Перегон»).
    pub relays: u64,
    /// Итог сделки, USDT (когда закрыта).
    pub result_usd: Option<Decimal>,
    /// Предварительный убыток, учтённый в итоге дня (решение 53, daily.rs): Halted или «Беру на себя»;
    /// None — не учитывался или уже заменён фактом.
    #[serde(default)]
    pub provisional_usd: Option<Decimal>,
    /// Хронология: время и что случилось — для экрана «Сделки».
    pub timeline: Vec<(DateTime<Utc>, String)>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Deal {
    /// Новая сделка по плану. Принимает: id, направление, план, плановую прибыль. Ошибок нет.
    pub fn new(id: String, direction: Direction, plan: Value, plan_profit_usd: Decimal) -> Self {
        let now = Utc::now();
        Deal {
            id,
            at: direction.buy_on.clone(),
            direction,
            plan,
            plan_profit_usd,
            step: Step::Buying,
            qty: Decimal::ZERO,
            invested: Decimal::ZERO,
            proceeds: Decimal::ZERO,
            spent: Decimal::ZERO,
            buy: None,
            transfers: vec![],
            sells: vec![],
            dust: Decimal::ZERO,
            relays: 0,
            result_usd: None,
            provisional_usd: None,
            timeline: vec![],
            started_at: now,
            updated_at: now,
        }
    }

    /// Запись в хронологию. Принимает: текст. Ошибок нет.
    pub fn note(&mut self, text: impl Into<String>) {
        self.updated_at = Utc::now();
        self.timeline.push((self.updated_at, text.into()));
    }

    /// Итог, если продать сейчас за `value` USDT (чистыми): выручка + value − вложено − потрачено.
    pub fn result_if(&self, value: Decimal) -> Decimal {
        self.proceeds + value - self.invested - self.spent
    }

    /// Убыток в процентах от вложенного, если продать за `value` (положительный — убыток).
    pub fn loss_pct_if(&self, value: Decimal) -> Decimal {
        if self.invested.is_zero() {
            return Decimal::ZERO;
        }
        -self.result_if(value) / self.invested * Decimal::ONE_HUNDRED
    }

    /// Открыта ли сделка (у бота есть её монеты или идёт шаг).
    pub fn is_open(&self) -> bool {
        !matches!(self.step, Step::Closed | Step::Halted | Step::TakenOver)
    }
}
