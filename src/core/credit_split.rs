//! Файл: core/credit_split.rs
//! Что делает: решение 413 (t198, живой случай WKC 18.11) — чистое распределение ФАКТА счёта биржи между сделками,
//! чьё зачисление ещё не подтверждено фактом («ждут доли»). Запись депозита биржи (по хешу перевода сделки) — привязка
//! депозита к сделке и ВЕРХНЯЯ граница; сколько монет сделки реально на счёте — по балансу.
//!
//! Входы (всё — монета на одном счёте биржи: торговый + перекладываемые на торговый, free + locked):
//! - fact — баланс счёта сейчас;
//! - confirmed — сумма учёта сделок, чьё количество уже подтверждено фактом (или куплено / продано на этой площадке);
//! - pending — ждущие доли: запись депозита и «посторонние» на счёте до вывода (outsider: баланс минус учёт сделок,
//!   снятый перед выводом, credit_base.rs; None — не снят);
//! - unclear — через этот счёт идут сделки на шаге «перевод» (их монеты могли уже прийти — не доказано);
//! - hold_pct — ожидаемое удержание биржи при зачислении, % (deposit_hold.rs: замер или, пока не измерено, налог
//!   перевода токена); tol_pct — допуск на округление (transfer.credit_amount_tolerance_pct).
//!
//! Правило (детерминированно, ни одна сделка не платит за другую, посторонние монеты не трогаются):
//! - посторонние U = наибольший из снятых у ждущих (не снят — 0, замер тогда «неоднозначный»);
//! - пул ждущих P = fact − confirmed − U; записи R = Σ записей;
//! - P ≥ R — каждой ровно её запись (лишнее P − R — не присваивается никому: round-up, airdrop, ручные — case 7);
//! - 0 < P < R — каждой запись × P / R (пропорционально записям: удержание — доля перевода, одинаковая у депозитов
//!   одной монеты на одной бирже), вниз до точности входных чисел; остаток округления не присваивается;
//! - P ≤ 0 — всем 0 (на счёте нет монет ждущих — решает человек, а не «фантом»);
//! - есть unclear — доля не больше запись × (1 − hold_pct / 100): в P могут быть монеты сделки в пути.
//!
//! Почему пропорционально, а не «по приращению баланса в момент зачисления»: приращение однозначно только когда между
//! двумя чтениями баланса зачислился ровно один депозит и ничего не продавалось; при двух депозитах между чтениями (или
//! продаже) оно не делится. Пропорция по записям — одна формула для обоих случаев и совпадает с приращением, когда оно
//! однозначно (одна ждущая — вся P её, не больше записи).
//! Кто вызывает: core/credit_apply.rs, тесты (tests/core_credit_fact_t198.rs).
//! От чего зависит: rust_decimal.
//! Настройки: нет (числа — из данных и существующих настроек вызывающего).

use rust_decimal::Decimal;

/// Ждущая доли сделка.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub id: String,
    /// Запись депозита биржи (сколько биржа записала по хешу перевода сделки).
    pub record: Decimal,
    /// Посторонние монеты на счёте перед выводом сделки (credit_base.rs); None — не сняты.
    pub outsider: Option<Decimal>,
}

/// Входы распределения (см. шапку).
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub fact: Decimal,
    pub confirmed: Decimal,
    pub pending: Vec<Pending>,
    pub unclear: bool,
    pub hold_pct: Decimal,
    pub tol_pct: Decimal,
}

/// Итог распределения.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    /// Доля каждой ждущей (в порядке входа).
    pub shares: Vec<(String, Decimal)>,
    /// Посторонние, вычтенные из факта.
    pub outsider: Decimal,
    /// Пул ждущих: fact − confirmed − outsider (может быть ≤ 0).
    pub pool: Decimal,
    /// Сумма записей ждущих.
    pub records: Decimal,
    /// Удержано биржей (записи − доли, не меньше 0) — монеты, которых по факту нет ни у одной ждущей.
    pub held: Decimal,
    /// Лишнее на счёте сверх записей (не присвоено).
    pub excess: Decimal,
    /// Удержание больше ожидаемого (с допуском): пул меньше записи × (1 − hold_pct / 100) × (1 − tol_pct / 100).
    pub over: bool,
    /// Замер однозначен: в пути через счёт никого, посторонние сняты у всех ждущих, записи > 0.
    pub measurable: bool,
}

impl Split {
    /// Доля ждущей сделки по ID. None — её нет среди ждущих. Ошибок нет.
    pub fn share_of(&self, id: &str) -> Option<Decimal> {
        self.shares.iter().find(|(x, _)| x == id).map(|(_, q)| *q)
    }

    /// Удержание в процентах от записей (0 — записей нет). Ошибок нет.
    pub fn held_pct(&self) -> Decimal {
        if self.records <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        self.held / self.records * Decimal::ONE_HUNDRED
    }
}

/// Что делает: распределение факта между ждущими (правило — шапка). Принимает: входы. Возвращает: Split. Ошибок нет.
pub fn split(i: &Input) -> Split {
    let outsider = i
        .pending
        .iter()
        .filter_map(|p| p.outsider)
        .fold(Decimal::ZERO, Decimal::max);
    let records: Decimal = i.pending.iter().map(|p| p.record.max(Decimal::ZERO)).sum();
    let pool = i.fact - i.confirmed - outsider;
    let keep = (Decimal::ONE - i.hold_pct / Decimal::ONE_HUNDRED).max(Decimal::ZERO);
    let scale = scale_of(i);
    let shares: Vec<(String, Decimal)> = i
        .pending
        .iter()
        .map(|p| {
            let r = p.record.max(Decimal::ZERO);
            let raw = if pool >= records {
                r
            } else if pool <= Decimal::ZERO || records.is_zero() {
                Decimal::ZERO
            } else {
                (r * pool / records).trunc_with_scale(scale)
            };
            let cap = if i.unclear {
                (r * keep).trunc_with_scale(scale)
            } else {
                r
            };
            (p.id.clone(), raw.min(cap).max(Decimal::ZERO))
        })
        .collect();
    let given: Decimal = shares.iter().map(|(_, q)| *q).sum();
    let tol = (Decimal::ONE - i.tol_pct / Decimal::ONE_HUNDRED).max(Decimal::ZERO);
    Split {
        outsider,
        held: (records - given).max(Decimal::ZERO),
        excess: (pool - records).max(Decimal::ZERO),
        over: pool < records * keep * tol,
        measurable: !i.unclear
            && records > Decimal::ZERO
            && i.pending.iter().all(|p| p.outsider.is_some()),
        shares,
        pool,
        records,
    }
}

/// Точность распределения — наибольшая точность входных чисел (факт, учёт, записи): доли не точнее данных и
/// одинаковы при одинаковых входах. Ошибок нет.
fn scale_of(i: &Input) -> u32 {
    i.pending
        .iter()
        .map(|p| p.record.scale())
        .chain([i.fact.scale(), i.confirmed.scale()])
        .max()
        .unwrap_or(0)
}
