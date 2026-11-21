//! Файл: report/cap_hold.rs
//! Что делает: решение 256 (дополнение — аудит отчёта, п. 4–6) — занятость денег сделки на площадке покупки для
//! столбцов «Занято …» листа «Сделки» и таблицы «Сколько держать на балансе». Только факты учёта сделки (core/deal.rs):
//! - площадка — где лежат деньги покупки: биржа покупки; покупка на DEX — кошелёк сделки (Deal.dex_account, решение
//!   132: у каждой биржи свой кошелёк DEX); кошелёк в сделке не записан — так и подписано;
//! - сумма — «Вложено» и только у сделок с покупкой (Deal.buy есть: core/fills.rs, у цикла — cycle_fact.rs);
//! - занято с — начало сделки (started_at; покупка — внутри сделки, не раньше);
//! - освобождены (до) — по фактам, варианты а)–е):
//! - а) сделка открыта — занято до выгрузки (пусто);
//! - б) остановлена / «Беру на себя» с монетами (Deal.qty > 0, как report/snap.rs holds) — занято до выгрузки; у
//!   «Беру на себя», возвращённой боту (сделка «<id>r», core/manual_return.rs), — как у сделки «<id>r»;
//! - в) возврат USDT на площадку покупки (решения 138, 227): все части «вернулись» / «перегнано вручную» — время
//!   последней записи хронологии «USDT вернулись…» / «Перегнано вручную (…» (usdt_return_job.rs,
//!   usdt_return_retry.rs, manual_usdt.rs; тексты с первой версии e58ffd7 / 4fbcdc6); часть «идёт» / «ждёт повтора»
//!   / «не вернулись» — занято до выгрузки (USDT не на площадке покупки);
//! - г) частей возврата нет — конец сделки (rows_time.rs ended_at): цикл DEX ↔ DEX (покупка и продажа на одной DEX —
//!   одна атомарная транзакция, решение 85); продано на площадке покупки (Deal.at = buy_on) —
//!   выручка уже там; иначе возврата у сделки нет (режим «отдельно», путь без возврата, сделка до 138) — выручка
//!   осталась на площадке продажи, после конца сделки это уже не деньги сделки (подписано в пояснении);
//! - д) конец сделки или время возврата в хронологии не найдены — в расчёт не входит (пометка, п. 5 аудита).
//! - сделка «Вернуть боту» («<id>r»): покупки нет — своя занятость не считается (вложено — доля исходной сделки, её
//!   занятость считается у исходной: п. 6 аудита).
//!
//! Кто вызывает: report/cap_rows.rs.
//! От чего зависит: core/deal.rs, core/usdt_return_state.rs, report/rows_time.rs (ended_at), notify/human.rs (venue).
//! Настройки: нет.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::deal::{Deal, Step};
use crate::core::usdt_return_state::UsdtReturn;
use crate::notify::human::venue;

use super::rows_time::ended_at;

/// Занятость сделки: площадка, учитывается ли, до какого времени (None — до выгрузки), пояснение.
#[derive(Debug, Clone, PartialEq)]
pub struct Hold {
    pub place: String,
    pub counted: bool,
    pub until: Option<DateTime<Utc>>,
    pub note: String,
}

/// Начало записи хронологии «вернулись» (все пути: задача возврата, повторы, «Перегнать»).
const RETURNED: &str = "USDT вернулись";
/// Начало записи хронологии «Перегнано вручную (кто): …».
const MANUAL: &str = "Перегнано вручную (";

/// Площадка денег покупки (см. шапку). Принимает: сделку, id площадок DEX, имена кошельков (id → имя). Ошибок нет.
pub fn place(d: &Deal, dex: &BTreeSet<String>, names: &BTreeMap<String, String>) -> String {
    let b = &d.direction.buy_on.0;
    if !dex.contains(b) {
        return venue(b);
    }
    match &d.dex_account {
        Some(a) => {
            let n = names.get(&a.0).cloned().unwrap_or_else(|| a.0.clone());
            format!("{}, кошелёк «{n}»", venue(b))
        }
        None => format!("{} (кошелёк в сделке не записан)", venue(b)),
    }
}

/// Занятость сделки (см. шапку). Принимает: сделку, все сделки по id, id площадок DEX, имена кошельков. Ошибок нет.
pub fn hold(
    d: &Deal,
    all: &HashMap<&str, &Deal>,
    dex: &BTreeSet<String>,
    names: &BTreeMap<String, String>,
) -> Hold {
    let place = place(d, dex, names);
    if d.buy.is_none() {
        let base = d.id.strip_suffix('r').filter(|b| all.contains_key(b));
        let note = match base {
            Some(b) => format!(
                "«Вернуть боту»: покупки нет, вложено — доля сделки {b} (занятость считается у неё)"
            ),
            None => "покупки не было — денег площадки не занимала".into(),
        };
        return Hold {
            place,
            counted: false,
            until: None,
            note,
        };
    }
    match release(d, all, true) {
        Ok((until, note)) => Hold {
            place,
            counted: true,
            until,
            note,
        },
        Err(note) => Hold {
            place,
            counted: false,
            until: None,
            note,
        },
    }
}

/// Когда деньги свободны: Ok((None, …)) — ещё заняты; Ok((Some(t), …)) — свободны с t; Err — не определить.
/// `follow` — идти ли к сделке «<id>r» (один шаг). Ошибок нет (Err — пояснение).
fn release(
    d: &Deal,
    all: &HashMap<&str, &Deal>,
    follow: bool,
) -> Result<(Option<DateTime<Utc>>, String), String> {
    if d.is_open() {
        return Ok((None, "сделка открыта — занято до выгрузки".into()));
    }
    if d.step == Step::TakenOver && follow {
        let rid = format!("{}r", d.id);
        if let Some(r) = all.get(rid.as_str()) {
            let (u, n) = release(r, all, false)?;
            return Ok((u, format!("«Беру на себя» → «Вернуть боту» {rid}: {n}")));
        }
    }
    if matches!(d.step, Step::Halted | Step::TakenOver) && d.qty > Decimal::ZERO {
        let who = if d.step == Step::Halted {
            "остановлена, монеты на площадке"
        } else {
            "«Беру на себя», монеты у человека"
        };
        return Ok((None, format!("{who} — занято до выгрузки")));
    }
    let end = ended_at(d)
        .ok_or("конец сделки в хронологии не найден — в расчёт одновременности не входит")?;
    let parts: Vec<&UsdtReturn> = d.usdt_return.iter().chain(&d.usdt_return_parts).collect();
    if parts.is_empty() {
        let note = if d.direction.buy_on == d.direction.sell_on {
            "цикл DEX ↔ DEX — деньги вернулись в кошелёк той же транзакцией; свободны с концом сделки"
        } else if d.at == d.direction.buy_on {
            "продано на площадке покупки — свободны с концом сделки"
        } else {
            "возврата USDT у сделки нет — выручка осталась на площадке продажи; свободны с концом сделки"
        };
        return Ok((Some(end), note.into()));
    }
    let back = |p: &&UsdtReturn| matches!(p, UsdtReturn::Done { .. } | UsdtReturn::Manual { .. });
    if !parts.iter().all(back) {
        return Ok((
            None,
            "USDT ещё не на площадке покупки (возврат идёт / ждёт повтора / не вернулись) — занято до выгрузки".into(),
        ));
    }
    let t = d
        .timeline
        .iter()
        .rev()
        .find(|(_, t)| t.starts_with(RETURNED) || t.starts_with(MANUAL))
        .map(|(at, _)| *at)
        .ok_or("USDT вернулись, но время возврата в хронологии не найдено — в расчёт одновременности не входит")?;
    Ok((
        Some(t.max(end)),
        "свободны с возвратом USDT на площадку покупки".into(),
    ))
}
