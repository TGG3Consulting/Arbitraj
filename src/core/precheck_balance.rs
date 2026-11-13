//! Файл: core/precheck_balance.rs
//! Что делает: решение 266 (Тигран 13.11.2026), часть 4 — свободный баланс A один раз, прямо перед последним расчётом
//! объёма (precheck.balance_first = да; изменяет решение 94 по указанию Тиграна 13.11). Раньше: предпроверка считала
//! план без баланса, после открытия сделки исполнитель читал баланс (limits::buy_cap) и, если его не хватало,
//! подбирал объём заново (refit.rs). Теперь баланс котируемой монеты A читается в предпроверке после ответов биржи и
//! перед расчётом — тем же кодом (limits::buy_free: биржа — баланс счёта, DEX — баланс кошелька сделки):
//! - баланс получен и его хватает кандидату — расчёт с пределом min(бюджет, свободно) (быстрый пересчёт объёма
//!   кандидата при precheck.fast_recalc = да, иначе полный подбор);
//! - баланс получен и его меньше списания кандидата (критерий refit::needed: списание по худшей цене больше предела) —
//!   сразу полный подбор под свободный остаток (profit::decide_capped с cap = свободно, как refit::plan); прошёл
//!   пороги — это финальный план; не прошёл — сделки нет, отказ предпроверки с причиной «свободный баланс» (сколько
//!   свободно, сколько нужно); сделка не открывается — паузы направления нет (Тигран 13.11: «пауза не нужна»);
//! - баланс получен и равен 0 — отказ предпроверки текстом buy_cap («на X свободно 0 …»);
//! - баланс НЕ получен (любая ошибка чтения): A — биржа — это не отказ (Тигран 13.11: «отправляем ордер на посчитанный
//!   объём: исполнится — отлично, нет — получим отказ из-за баланса»): расчёт с пределом = бюджет, ордер уходит без
//!   проверки баланса, в хронологию и журнал core.precheck — строка missed_note; A — DEX — отказ предпроверки с
//!   причиной (что покупка на DEX при нехватке баланса всегда отсекается до отправки транзакции, по коду не доказано).
//!
//! Исполнитель при этом режиме баланс не читает и объём под остаток не подбирает (executor.rs): предел списания ордера —
//! min(бюджет плана, свободно) по прочитанному здесь.
//! Кто вызывает: core/precheck.rs (steps), core/precheck_par.rs (чтение одновременно с запросами сторон),
//! core/executor.rs (Bal — заметки хронологии и предел списания).
//! От чего зависит: core/limits.rs (buy_free, none_free), trade_ops.rs (вид площадки).
//! Настройки: precheck.balance_first (читает executor.rs через precheck_fast.rs read); venue.quote_asset (limits.rs).

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, Direction};
use crate::gateway::contract::VenueKind;

use super::costs::Calc;
use super::engine::Core;

/// Включатель части 4.
pub const BALANCE: &str = "precheck.balance_first";

/// Числа кандидата, которые нужны части 4: объём, списание по худшей цене, прибыль.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cand {
    pub qty: Decimal,
    pub spend_max: Decimal,
    pub profit_usd: Decimal,
}

impl Cand {
    /// Из расчёта кандидата. Ошибок нет.
    pub fn of(c: &Calc) -> Self {
        Cand {
            qty: c.qty,
            spend_max: c.spend_max,
            profit_usd: c.profit_usd,
        }
    }

    /// Свободного меньше, чем нужно кандидату, — тот же критерий, что refit::needed (списание по худшей цене больше
    /// предела). Принимает: свободный баланс. Ошибок нет.
    pub fn short(&self, free: Decimal) -> bool {
        self.spend_max > free
    }
}

/// Итог чтения баланса в предпроверке.
#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    /// Баланс получен: свободно (> 0), тикер котируемой монеты A.
    Got { free: Decimal, quote: String },
    /// Биржа A баланс не отдала (причина) — ордер пойдёт на посчитанный объём без проверки баланса.
    Missed(String),
}

impl Read {
    /// Свободный баланс, если получен. Ошибок нет.
    pub fn free(&self) -> Option<Decimal> {
        match self {
            Read::Got { free, .. } => Some(*free),
            Read::Missed(_) => None,
        }
    }
}

/// Что предпроверка передаёт исполнителю при precheck.balance_first = да.
#[derive(Debug, Clone, PartialEq)]
pub struct Bal {
    pub read: Read,
    /// План считался под свободный остаток (свободного меньше списания кандидата) — числа кандидата для заметки.
    pub refit: Option<Cand>,
}

/// Что делает: читает свободный баланс котируемой монеты A тем же кодом, что buy_cap. Принимает: ядро, направление,
/// счёт DEX сделки. Возвращает: Read. Ошибки (отказ предпроверки): свободно 0 (текст buy_cap); A — DEX и баланс не
/// получен (причина чтения).
pub async fn read(core: &Core, d: &Direction, acc: Option<&DexAccountId>) -> Result<Read, String> {
    match super::limits::buy_free(core, d, acc).await {
        Ok((free, quote)) if free <= Decimal::ZERO => {
            Err(super::limits::none_free(&d.buy_on, &quote))
        }
        Ok((free, quote)) => Ok(Read::Got { free, quote }),
        Err(why) if super::trade_ops::kind(core, &d.buy_on) == Ok(VenueKind::Cex) => {
            Ok(Read::Missed(why))
        }
        Err(why) => Err(format!("свободный баланс {} не получен: {why}", d.buy_on.0)),
    }
}

/// Строка хронологии сделки и журнала core.precheck: биржа A баланс не отдала. Принимает: направление, причину.
/// Ошибок нет.
pub fn missed_note(d: &Direction, why: &str) -> String {
    format!(
        "Свободный баланс на {} не получен: {why} — ордер на посчитанный объём без проверки баланса (решение 266)",
        d.buy_on.0
    )
}

/// Текст отказа предпроверки: свободного меньше, чем нужно кандидату, и подбор под свободный остаток не дал плана.
/// Принимает: направление, свободно, котируемую монету, числа кандидата, причину отказа расчёта. Ошибок нет.
pub fn short_reject(d: &Direction, free: Decimal, quote: &str, c: &Cand, why: &str) -> String {
    format!(
        "свободный баланс {}: свободно {free} {quote}, кандидату нужно {} — на свободный остаток {free} сделка \
         невыгодна: {why}",
        d.buy_on.0,
        c.spend_max.round_dp(4)
    )
}
