//! Файл: report/cap_episodes.rs
//! Что делает: решение 256 — строки листа «Упущено из-за потолка» отчёта «Сделки, лимиты и капитал»: записи «без
//! потолка» периода (те же, что у экрана и отчёта «Упущено»: core/uncapped_sum.rs totals — кандидаты с упущено 0 скрыты,
//! у сделки одна запись, последняя) сводятся в ЭПИЗОДЫ, эпизоды — в строки «день × направление».
//! Правила:
//! - эпизод = направление (монета по адресу + биржа покупки + биржа продажи) + начало эпизода (Brief.ep.first_at);
//! - записи сделок не по данным момента входа (Brief::incomparable, решение 239) не входят — как в таблицу потолков
//!   экрана; их число — в примечании листа «Итоги»;
//! - по эпизоду берётся НАИБОЛЬШАЯ прибыль на нашем бюджете и на каждом потолке (uncapped_types.rs take: точка проходит
//!   пороги — её прибыль, иначе 0); потолок, которого нет ни в одной записи эпизода (записи до смены
//!   uncapped.ladder_usdt), — пусто, а не 0;
//! - «что ограничило» эпизода — у его пиковой записи (наибольшее упущено, как пик эпизода в core/uncapped_store.rs;
//!   равные — первая): optimum → «выгодный объём», per_trade → «лимит на сделку», free → «свободный баланс»,
//!   liquidity → «ликвидность», per_coin → «лимит на монету», остальное (in_transit, dry, budget, search_cap) → «другое»;
//! - эпизод со сделкой — в нём есть запись исполненной сделки (Item.deal);
//! - день — дата начала эпизода (UTC); направление — «MEXC → DEX» (notify/human.rs route).
//!
//! Кто вызывает: report/mod.rs (capital_send), tests/report_capital.rs.
//! От чего зависит: core/uncapped_sum.rs (Totals, Item), core/uncapped_types.rs (take), notify/human.rs (route).
//! Настройки: нет (потолки — uncapped.ladder_usdt, приходят готовым списком).

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;

use crate::core::uncapped_sum::{Item, Totals};
use crate::core::uncapped_types::take;
use crate::notify::human::route;

use super::cap_rows::f;

/// Столбцы «сколько раз ограничил» по порядку листа.
pub const LIMITS: [&str; 6] = [
    "выгодный объём",
    "лимит на сделку",
    "свободный баланс",
    "ликвидность",
    "лимит на монету",
    "другое",
];

/// Строка «день × направление».
#[derive(Debug, Clone, PartialEq)]
pub struct DayRow {
    pub day: NaiveDate,
    pub dir: String,
    pub episodes: u64,
    pub with_deal: u64,
    pub at_budget: f64,
    /// По потолкам из списка (порядок списка); None — ни у одного эпизода строки нет этого потолка.
    pub caps: Vec<Option<f64>>,
    /// Сколько эпизодов ограничил каждый столбец LIMITS.
    pub limits: [u64; 6],
}

/// Итог сводки: строки и сколько записей сделок не вошло (решение 239).
pub struct Episodes {
    pub rows: Vec<DayRow>,
    pub incomparable: u64,
}

/// Индекс столбца LIMITS по «что ограничило» записи. Ошибок нет.
pub fn limit_index(limited_by: &str) -> usize {
    match limited_by {
        "optimum" => 0,
        "per_trade" => 1,
        "free" => 2,
        "liquidity" => 3,
        "per_coin" => 4,
        _ => 5,
    }
}

/// Один эпизод: записи в порядке выборки.
struct Ep<'a> {
    items: Vec<&'a Item>,
}

impl Ep<'_> {
    /// Наибольшая прибыль на потолке среди записей; None — потолка нет ни в одной записи. Ошибок нет.
    fn cap(&self, cap: Decimal) -> Option<Decimal> {
        self.items
            .iter()
            .filter_map(|i| i.b.levels.iter().find(|l| l.cap == cap).map(take))
            .max()
    }

    /// Пиковая запись — наибольшее упущено (равные — первая). Ошибок нет (эпизод не пуст).
    fn peak(&self) -> &Item {
        let mut best = self.items[0];
        for i in &self.items[1..] {
            if i.b.missed > best.b.missed {
                best = i;
            }
        }
        best
    }
}

/// Что делает: сводка эпизодов (см. шапку). Принимает: итоги периода, потолки uncapped.ladder_usdt. Возвращает: строки
/// по дню, затем по направлению (по алфавиту), и число не вошедших записей сделок. Ошибок нет.
pub fn episodes(t: &Totals, ladder: &[Decimal]) -> Episodes {
    let mut incomparable = 0;
    // Ключ эпизода: направление строкой (монета по адресу или тикеру + площадки) и начало.
    let mut eps: BTreeMap<(String, DateTime<Utc>), Ep<'_>> = BTreeMap::new();
    for i in &t.items {
        if i.deal && i.b.incomparable() {
            incomparable += 1;
            continue;
        }
        let d = &i.b.direction;
        let coin = match d.asset.bsc_contract {
            Some(a) => format!("{a:#x}"),
            None => format!("ticker:{}", d.asset.ticker),
        };
        let key = format!("{coin}|{}|{}", d.buy_on.0, d.sell_on.0);
        eps.entry((key, i.b.ep.first_at))
            .or_insert_with(|| Ep { items: vec![] })
            .items
            .push(i);
    }
    let mut rows: BTreeMap<(NaiveDate, String), DayRow> = BTreeMap::new();
    for ((_, first), ep) in &eps {
        let d = &ep.items[0].b.direction;
        let dir = route(&d.buy_on.0, &d.sell_on.0);
        let day = first.date_naive();
        let r = rows.entry((day, dir.clone())).or_insert_with(|| DayRow {
            day,
            dir,
            episodes: 0,
            with_deal: 0,
            at_budget: 0.0,
            caps: vec![None; ladder.len()],
            limits: [0; 6],
        });
        r.episodes += 1;
        r.with_deal += u64::from(ep.items.iter().any(|i| i.deal));
        let ab = ep.items.iter().map(|i| take(&i.b.at_budget)).max();
        r.at_budget += f(ab.unwrap_or_default());
        for (k, cap) in ladder.iter().enumerate() {
            if let Some(v) = ep.cap(*cap) {
                *r.caps[k].get_or_insert(0.0) += f(v);
            }
        }
        r.limits[limit_index(&ep.peak().b.limited_by)] += 1;
    }
    Episodes {
        rows: rows.into_values().collect(),
        incomparable,
    }
}
