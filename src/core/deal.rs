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
    /// Решение 101: после продажи выручку USDT перегнать на площадку покупки (кнопка «Продать и перегнать»),
    /// независимо от usdt_return.mode.
    #[serde(default)]
    pub return_usdt: bool,
    /// Решение 110: сколько раз продажа не ушла в сеть и монета вернулась в режим застрявшей (предел —
    /// stuck.unsent_retries_max, дальше — алерт человеку).
    #[serde(default)]
    pub unsent_retries: u32,
    /// Решение 229: цепочку повторов продажи начал случай решения 229 (продажа не попала в блок, а отмена подтверждена;
    /// отказ до отправки после кнопки человека) — исчерпание повторов ведёт к продаже здесь по порогу
    /// stuck.relay_max_loss_pct (sell_unsent.rs → relay_here.rs), а не сразу к человеку.
    #[serde(default)]
    pub unsent_here: bool,
    /// Итог сделки, USDT (когда закрыта): выручка − вложено − затраты − затраты возврата USDT (return_cost_usd).
    pub result_usd: Option<Decimal>,
    /// Решение 236: затраты возврата выручки USDT, уже вычтенные из result_usd, — по частям «вернулись»: отправлено −
    /// зачислено + газ/комиссия не в USDT (deal_return_cost.rs). None — ещё не считались (старая запись: считаются при
    /// загрузке в памяти).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_cost_usd: Option<Decimal>,
    /// Решение 236: уведомление deal.closed ждёт конца возврата USDT (факт — с его затратами); текст — причина
    /// закрытия. None — не ждёт (отправлено или возврата нет).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_notice: Option<String>,
    /// Предварительный убыток, учтённый в итоге дня (решение 53, daily.rs): Halted или «Беру на себя»;
    /// None — не учитывался или уже заменён фактом.
    #[serde(default)]
    pub provisional_usd: Option<Decimal>,
    /// Решение 132: счёт (кошелёк) DEX сделки — по маппингу на момент открытия; все шаги сделки идут им, даже если
    /// маппинг потом поменяли. None — без DEX (биржа ↔ биржа) или сделка до решения 132 (тогда — кошелёк переноса).
    #[serde(default)]
    pub dex_account: Option<crate::core::types::DexAccountId>,
    /// Решение 138: возврат выручки USDT на площадку покупки после закрытия (usdt_return_deal.rs): идёт / вернулись /
    /// не вернулись. None — возврат не нужен (продано на площадке покупки, режим «отдельно») или сделка до решения 138.
    #[serde(default)]
    pub usdt_return: Option<super::usdt_return_state::UsdtReturn>,
    /// Решение 227: остальные части возврата — выручка продаж на других площадках (кроме площадки покупки), каждая своим
    /// переводом (usdt_return_parts.rs); usdt_return — первая часть. Возврат закончен, когда закончены все части.
    /// Пусто — одна часть (и у сделок до решения 227).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usdt_return_parts: Vec<super::usdt_return_state::UsdtReturn>,
    /// Решение 138: путь закрытия, на котором возврата по решению нет (например «Продано вручную», решение 100), —
    /// причина; при закрытии вместо перегона — страховка «USDT не вернулись». None — обычный путь.
    #[serde(default)]
    pub usdt_return_skip: Option<String>,
    /// Решение 227 (дополнение): площадки, отказавшие в выводе в этой сделке (WithdrawRejected / InvalidAddress /
    /// WithdrawLocked, withdraw_refused.rs) — «Перегон» с них больше не выбирается. Пусто — отказов не было.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withdraw_refused: Vec<VenueId>,
    /// Решение 177: алерт, с которого человек запустил продажу («Продать сейчас», «Продать и перегнать»); тот же отказ —
    /// эта запись обновляется (core/manual_repeat.rs), а не ставится новая. None — продажа не с алерта.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert_was: Option<crate::core::types::StuckAlert>,
    /// Хронология: время и что случилось — для экрана «Сделки».
    pub timeline: Vec<(DateTime<Utc>, String)>,
    /// Решение 245, дополнение п. 2 (Тигран 09.11: согласовано): момент решения о сделке — время расчёта кандидата,
    /// прошедшего пороги прибыли (Calc.at вердикта Accept, по которому исполнитель запущен: executor.rs). Длительность
    /// сделки для «успели бы» (deal_time.rs) — от него. None — сделка до дополнения или не с пути кандидата (цикл
    /// DEX ↔ DEX, ручные действия): берётся started_at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
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
            return_usdt: false,
            unsent_retries: 0,
            unsent_here: false,
            result_usd: None,
            return_cost_usd: None,
            closed_notice: None,
            provisional_usd: None,
            dex_account: None,
            usdt_return: None,
            usdt_return_parts: vec![],
            usdt_return_skip: None,
            withdraw_refused: vec![],
            alert_was: None,
            timeline: vec![],
            decided_at: None,
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
