//! Файл: core/credit_halted.rs
//! Что делает: решение 413 (повторное ревью t198, С-5; третье ревью, С-Б) — выход из «ждёт доли» на круге сверки
//! балансов для сделок без своей задачи продажи: остановленных (в том числе после перезапуска) и ждущих человека. Доля —
//! тем же распределением (credit_split.rs) по балансу монеты, который уже прочитал этот круг сверки
//! (reconcile_key::total — без отдельного чтения), и только по монетам, чьи такие сделки не менялись за время запроса
//! баланса (prints до и после — как решение 112). Сохранение — проверка «шаг и время последнего события те же» и запись
//! в учёт памяти под одним замком учёта (действие человека не перетирается), затем ledger::save.
//! Удержано больше ожидаемого (over) — долю НЕ подтверждаем (круг видит только торговый счёт, монеты на счёте перевода —
//! нет; решит факт всех счетов перед продажей): журнал core.credit_fact Warn и Telegram deal.credit_hold — один раз на
//! (сделка, баланс), не каждый круг; доля по факту запоминается (observed) — сверка (reconcile_confirm.rs) считает эту
//! ждущую ею, а не нижней оценкой, чтобы не ставить паузу биржи (повтор WKC-паузы недопустим).
//! Кто вызывает: core/reconcile.rs (prints, stable, confirm), core/reconcile_confirm.rs (observed).
//! От чего зависит: credit_apply.rs (input), credit_split.rs, credit_text.rs, credit_alert.rs (notice), credit_fact.rs,
//! remain_others.rs, reconcile_key.rs, ledger.rs, engine.rs.
//! Настройки: нет.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::credit_fact::Basis;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Отпечатки ждущих без задачи: (ключ монеты, ID, шаг, время последнего события).
pub type Prints = Vec<(String, String, Step, DateTime<Utc>)>;

/// Уже сообщённые «больше ожидаемого»: (сделка, баланс).
static NOTED: LazyLock<Mutex<HashSet<(String, Decimal)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Доля по факту у ждущих с «больше ожидаемого»: ID → доля.
static OBSERVED: LazyLock<Mutex<HashMap<String, Decimal>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Ждущая доли сделка без задачи продажи на бирже `v`: остановлена или ждёт человека. Ошибок нет.
fn idle(d: &Deal, v: &VenueId) -> bool {
    &d.at == v
        && matches!(d.step, Step::Halted | Step::WaitingHuman)
        && super::credit_fact::pending(d)
}

/// Что делает: отпечатки ждущих без задачи на бирже (до и после запроса баланса круга). Ошибок нет.
pub fn prints(core: &Core, v: &VenueId) -> Prints {
    let mut out: Prints = lk(&core.inner.deals)
        .values()
        .filter(|d| idle(d, v))
        .map(|d| {
            let k = super::reconcile_key::key(core, &d.direction.asset, v);
            (k, d.id.clone(), d.step, d.updated_at)
        })
        .collect();
    out.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    out
}

/// Что делает: ключи монет, у которых отпечатки до и после одинаковы (стабильные). Ошибок нет.
pub fn stable(before: &Prints, after: &Prints) -> Vec<String> {
    let keys: HashSet<&String> = after.iter().map(|p| &p.0).collect();
    let mut out: Vec<String> = keys
        .into_iter()
        .filter(|k| {
            let a: Vec<_> = before.iter().filter(|p| &&p.0 == k).collect();
            let b: Vec<_> = after.iter().filter(|p| &&p.0 == k).collect();
            a == b
        })
        .cloned()
        .collect();
    out.sort();
    out
}

/// Доля по факту ждущей с «больше ожидаемого» (для сверки). None — не наблюдалось. Ошибок нет.
pub fn observed(id: &str) -> Option<Decimal> {
    OBSERVED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .copied()
}

/// Что делает: подтверждение долей ждущих без задачи монеты `key` на бирже `v` по балансу круга `fact` (шапка).
/// Принимает: ядро, биржу, ключ монеты сверки, баланс. Ошибок нет.
pub async fn confirm(core: &Core, v: &VenueId, key: &str, fact: Decimal) {
    let list: Vec<Deal> = lk(&core.inner.deals)
        .values()
        .filter(|d| idle(d, v) && super::reconcile_key::key(core, &d.direction.asset, v) == key)
        .cloned()
        .collect();
    for mut d in list {
        let o = super::remain_others::on_account(core, &d, v);
        let sp = super::credit_split::split(&super::credit_apply::input(core, &d, fact, &o));
        let share = sp.share_of(&d.id).unwrap_or_default();
        let record = d.qty;
        let text = super::credit_text::describe(core, &d, fact, &o, &sp, (record, share));
        if sp.over {
            Box::pin(over(core, &d, v, (fact, record, share), &text)).await;
            continue;
        }
        if share <= Decimal::ZERO {
            continue;
        }
        let seen = (d.step, d.updated_at);
        super::credit_fact::confirm(&mut d, share, Basis::Balance);
        d.note(format!("{text} (подтверждено на круге сверки балансов)"));
        // Проверка и запись в учёт памяти — под одним замком учёта.
        let written = {
            let mut m = lk(&core.inner.deals);
            let same = m.get(&d.id).is_some_and(|x| (x.step, x.updated_at) == seen);
            if same {
                m.insert(d.id.clone(), d.clone());
            }
            same
        };
        if !written {
            continue;
        }
        OBSERVED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&d.id);
        super::ledger::save(core, &d).await;
        core.inner
            .deps
            .log(Level::Info, "core.credit_fact", Some(&v.0), &text, None)
            .await;
    }
}

/// «Больше ожидаемого» на круге сверки: доля запоминается для сверки; журнал и Telegram — один раз на (сделка, баланс).
async fn over(
    core: &Core,
    d: &Deal,
    v: &VenueId,
    (fact, record, share): (Decimal, Decimal, Decimal),
    text: &str,
) {
    OBSERVED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(d.id.clone(), share);
    let first = NOTED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert((d.id.clone(), fact));
    if !first {
        return;
    }
    let deps = &core.inner.deps;
    let t = format!(
        "{text} — на круге сверки не подтверждено: удержано больше ожидаемого, доля — по факту всех счетов перед \
         продажей; в сверке эта сделка считается долей по факту"
    );
    deps.log(Level::Warn, "core.credit_fact", Some(&v.0), &t, None)
        .await;
    if let Some(tg) = super::credit_alert::notice(d, record, share) {
        deps.notify("deal.credit_hold", Some(&v.0), &tg).await;
    }
}
