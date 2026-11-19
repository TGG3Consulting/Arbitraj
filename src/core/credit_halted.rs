//! Файл: core/credit_halted.rs
//! Что делает: решение 413 (повторное ревью t198, С-5; третье, С-Б; четвёртое, С-2) — круг сверки балансов и сделки,
//! ждущие доли без своей задачи продажи (остановленные, ждущие человека). Круг видит только торговый счёт и не держит
//! замок счёта, поэтому долю он НЕ подтверждает (иначе занижал бы её навсегда: монеты на счёте перевода, продажа
//! другой сделки в полёте) — окончательная доля только перед продажей / выводом под замком счёта (sell_fact::before).
//! Круг лишь запоминает долю по его балансу (observed) — сверка (reconcile_confirm.rs) считает ждущую ею, а не нижней
//! оценкой, чтобы удержание биржи не ставило паузу площадки (повтор WKC-паузы недопустим); только по монетам, чьи
//! такие сделки не менялись за запрос баланса (prints до и после — как решение 112). Удержано больше ожидаемого —
//! журнал core.credit_fact Warn и Telegram deal.credit_hold один раз на (сделка, биржа, запись, баланс).
//! Память — по ключу (сделка, биржа, запись) и чистится, когда сделка перестала ждать доли на этой бирже (М-4).
//! Кто вызывает: core/reconcile.rs (prints, stable, observe), core/reconcile_confirm.rs (observed).
//! От чего зависит: credit_apply.rs (input), credit_split.rs, credit_text.rs, credit_alert.rs (notice), credit_fact.rs,
//! remain_others.rs, reconcile_key.rs, engine.rs.
//! Настройки: нет.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Отпечатки ждущих без задачи: (ключ монеты, ID, шаг, время последнего события).
pub type Prints = Vec<(String, String, Step, DateTime<Utc>)>;

/// Ключ памяти: (сделка, биржа, запись депозита).
type Key = (String, VenueId, Decimal);

/// Уже сообщённые «больше ожидаемого»: ключ → балансы, о которых сообщено.
static NOTED: LazyLock<Mutex<HashMap<Key, HashSet<Decimal>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Доля по балансу круга сверки: ключ → доля.
static OBSERVED: LazyLock<Mutex<HashMap<Key, Decimal>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Ключ памяти сделки, ждущей доли; None — не ждёт. Ошибок нет.
fn key_of(d: &Deal) -> Option<Key> {
    let c = super::credit_fact::here(d).filter(|_| super::credit_fact::pending(d))?;
    Some((d.id.clone(), d.at.clone(), c.record))
}

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
            let a: Vec<_> = before.iter().filter(|p| &p.0 == *k).collect();
            let b: Vec<_> = after.iter().filter(|p| &p.0 == *k).collect();
            a == b
        })
        .cloned()
        .collect();
    out.sort();
    out
}

/// Доля по балансу круга сверки у ждущей доли сделки (для сверки). None — не наблюдалась. Ошибок нет.
pub fn observed(d: &Deal) -> Option<Decimal> {
    let k = key_of(d)?;
    OBSERVED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&k)
        .copied()
}

/// Что делает: память по сделкам, переставшим ждать доли (М-4) — забыть. Принимает: ядро. Ошибок нет.
fn forget_gone(core: &Core) {
    let live: HashSet<Key> = lk(&core.inner.deals).values().filter_map(key_of).collect();
    OBSERVED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|k, _| live.contains(k));
    NOTED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|k, _| live.contains(k));
}

/// Что делает: доли ждущих без задачи монеты `key` на бирже `v` по балансу круга `fact` — только запомнить для сверки;
/// «больше ожидаемого» — сообщить один раз (шапка). Принимает: ядро, биржу, ключ монеты сверки, баланс. Ошибок нет.
pub async fn observe(core: &Core, v: &VenueId, key: &str, fact: Decimal) {
    forget_gone(core);
    let list: Vec<Deal> = lk(&core.inner.deals)
        .values()
        .filter(|d| idle(d, v) && super::reconcile_key::key(core, &d.direction.asset, v) == key)
        .cloned()
        .collect();
    for d in list {
        let Some(k) = key_of(&d) else { continue };
        let o = super::remain_others::on_account(core, &d, v);
        let sp = super::credit_split::split(&super::credit_apply::input(core, &d, fact, &o));
        let share = sp.share_of(&d.id).unwrap_or_default();
        OBSERVED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(k.clone(), share);
        if !sp.over {
            continue;
        }
        let first = NOTED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(k)
            .or_default()
            .insert(fact);
        if first {
            let text = super::credit_text::describe(core, &d, fact, &o, &sp, (d.qty, share));
            Box::pin(over(core, &d, v, (d.qty, share), &text)).await;
        }
    }
}

/// «Больше ожидаемого» на круге сверки: журнал Warn и Telegram deal.credit_hold. Ошибок нет.
async fn over(core: &Core, d: &Deal, v: &VenueId, (record, share): (Decimal, Decimal), text: &str) {
    let deps = &core.inner.deps;
    let t = format!(
        "{text} — круг сверки (только торговый счёт, без замка) долю не подтверждает: удержано больше ожидаемого, доля \
         — по факту всех счетов перед продажей; в сверке сделка считается этой долей"
    );
    deps.log(Level::Warn, "core.credit_fact", Some(&v.0), &t, None)
        .await;
    if let Some(tg) = super::credit_alert::notice(d, record, share) {
        deps.notify("deal.credit_hold", Some(&v.0), &tg).await;
    }
}
