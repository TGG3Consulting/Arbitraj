//! Файл: report/missed_rows.rs
//! Что делает: решение 215 — строки отчёта «Упущено» (arb_missed_<дата>.xlsx) из итогов экрана «Упущено из-за потолка»
//! (core/uncapped_sum.rs Totals): строка на исполненную сделку (лист «Сделки»), строка на эпизод кандидата (лист
//! «Кандидаты»), направления по упущенному (лист «Топ направлений»). Значения — те же поля записей, что показывает
//! экран (ui/src/pages/Uncapped.svelte): сделка — время расчёта после покупки, зашли на (вложено боя), что ограничило,
//! план, факт из учёта, без потолка вложить → прибыль, упущено, окно от спреда выше порога до входа; кандидат — эпизод
//! (первый → последний расчёт с упущенным), вердикт и причина, расчётов, упёрлись в бюджет, наш бюджет, без потолка,
//! минимум до плюса, упущено; решение 245 — окно максимального объёма, длительность сделок пары, «успели бы»
//! (missed_maxwin.rs). Подписи «что ограничило» и вердикта — как ui/src/lib/uncappedLabels.js (LIMITED,
//! VERDICT; report/missed_labels.rs); площадки — человеческие имена (notify/human.rs venue); монета — показное имя по адресу (решение 211).
//! Порядок строк — по времени, старые сверху (как «Итоги сделок»). Время — UTC.
//! Решение 217: сделки, посчитанные после нашей покупки (старые записи, занижено), — отдельными строками (deals old).
//! Решение 239: отдельно — все записи сделок не по данным момента входа (Brief::incomparable), лист «Сделки — несравнимо».
//! Кто вызывает: report/missed_xlsx.rs.
//! От чего зависит: core/uncapped_report.rs (MissedData), uncapped_sum.rs (Item), uncapped_types.rs (Brief),
//! notify/human.rs (venue).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::types::Direction;
use crate::core::uncapped_report::MissedData;
use crate::core::uncapped_sum::{deal_end, window_s};
use crate::notify::human::venue;

pub use super::missed_labels::{basis_label, limited_label, verdict_label};

/// Строка листа «Сделки».
#[derive(Debug, Clone, PartialEq)]
pub struct DealRow {
    pub time: String,
    pub coin: String,
    pub dir: String,
    pub invested: f64,
    pub limited: String,
    pub plan: f64,
    /// Итог сделки из учёта; None — ещё нет.
    pub fact: Option<f64>,
    /// Решение 237: прибыль той же модели на вложенном объёме (Record.at_budget у сделки); None — точки нет.
    pub at_invested: Option<f64>,
    pub opt_invested: f64,
    pub opt_profit: f64,
    pub missed: f64,
    /// «начало окна → момент решения о входе (N с)» (старые записи — расчёт после покупки); пусто — начала окна нет.
    pub window: String,
    /// Решение 217: на каких данных посчитано (basis_label).
    pub basis: String,
}

/// Строка листа «Кандидаты».
#[derive(Debug, Clone, PartialEq)]
pub struct CandRow {
    pub start: String,
    pub end: String,
    pub coin: String,
    pub dir: String,
    pub verdict: String,
    pub evals: u64,
    pub capped: u64,
    pub budget: f64,
    pub opt_invested: f64,
    pub opt_profit: f64,
    /// Минимальное вложение до плюса; None — «не становится плюсовой».
    pub min_plus: Option<f64>,
    pub missed: f64,
    /// Решение 245: окно максимального объёма эпизода, длительность сделок, «успели бы».
    pub maxwin: super::missed_maxwin::MaxWinCols,
}

/// Строка листа «Топ направлений».
#[derive(Debug, Clone, PartialEq)]
pub struct TopRow {
    pub coin: String,
    pub dir: String,
    pub count: u64,
    pub missed: f64,
}

/// Число Decimal, округлённое до `dp` знаков, в f64 (как report/rows.rs). Ошибок нет (не влезло — 0).
pub fn num(d: Decimal, dp: u32) -> f64 {
    d.round_dp(dp).to_f64().unwrap_or(0.0)
}

/// Время «ГГГГ-ММ-ДД ЧЧ:ММ:СС» (UTC). Ошибок нет.
pub fn when(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// «BingX → DEX». Ошибок нет.
fn dir_text(d: &Direction) -> String {
    format!("{} → {}", venue(&d.buy_on.0), venue(&d.sell_on.0))
}

/// Показное имя монеты направления (нет в справочнике — тикер). Ошибок нет.
fn coin(data: &MissedData, d: &Direction) -> String {
    data.labels
        .get(&d.asset)
        .cloned()
        .unwrap_or_else(|| d.asset.ticker.clone())
}

/// Строки листа «Сделки» (old = false) или «Сделки — несравнимо» (old = true: посчитаны не по данным момента входа,
/// решение 239), старые сверху. Ошибок нет.
pub fn deals(data: &MissedData, old: bool) -> Vec<DealRow> {
    let mut v: Vec<DealRow> = data
        .totals
        .items
        .iter()
        .rev()
        .filter(|i| i.deal && i.b.incomparable() == old)
        .map(|i| {
            let b = &i.b;
            let window = match (b.battle.window_from, window_s(b)) {
                (Some(f), Some(s)) => {
                    format!("{} → {} ({s} с)", when(f), deal_end(b).format("%H:%M:%S"))
                }
                _ => String::new(),
            };
            DealRow {
                time: when(b.at),
                coin: coin(data, &b.direction),
                dir: dir_text(&b.direction),
                invested: num(b.battle.invested, 2),
                limited: limited_label(&b.limited_by),
                plan: num(b.battle.profit, 4),
                fact: i.fact.map(|f| num(f, 4)),
                at_invested: b.at_budget.at.map(|a| num(a.profit, 4)),
                opt_invested: num(b.opt.invested, 2),
                opt_profit: num(b.opt.profit, 4),
                missed: num(b.missed, 4),
                window,
                basis: basis_label(&b.basis),
            }
        })
        .collect();
    v.sort_by(|a, b| a.time.cmp(&b.time));
    v
}

/// Строки листа «Кандидаты» (эпизоды, старые сверху). Ошибок нет.
pub fn candidates(data: &MissedData) -> Vec<CandRow> {
    let mut v: Vec<CandRow> = data
        .totals
        .items
        .iter()
        .filter(|i| !i.deal)
        .map(|i| {
            let b = &i.b;
            let mut verdict = verdict_label(&b.battle.verdict);
            if let Some(r) = &b.battle.reason {
                verdict.push_str(&format!(": {r}"));
            }
            CandRow {
                start: when(b.ep.first_at),
                end: when(b.ep.last_at),
                coin: coin(data, &b.direction),
                dir: dir_text(&b.direction),
                verdict,
                evals: b.ep.count,
                capped: b.ep.capped_count,
                budget: num(b.budget, 2),
                opt_invested: num(b.opt.invested, 2),
                opt_profit: num(b.opt.profit, 4),
                min_plus: b.min_plus.as_ref().map(|m| num(m.invested, 2)),
                missed: num(b.missed, 4),
                maxwin: {
                    let (d, w) = (&b.direction, b.maxwin.as_ref());
                    super::missed_maxwin::cols(&data.maxwin.view(&d.buy_on, &d.sell_on, w), w)
                },
            }
        })
        .collect();
    v.sort_by(|a, b| a.start.cmp(&b.start));
    v
}

/// Строки листа «Топ направлений»: все направления по убыванию упущенного. Ошибок нет.
pub fn top(data: &MissedData) -> Vec<TopRow> {
    data.totals
        .top
        .iter()
        .map(|t| TopRow {
            coin: coin(data, &t.dir),
            dir: dir_text(&t.dir),
            count: t.count,
            missed: num(t.missed, 4),
        })
        .collect()
}
