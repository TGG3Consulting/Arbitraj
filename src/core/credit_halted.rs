//! Файл: core/credit_halted.rs
//! Что делает: решение 413 (повторное ревью t198, С-5) — выход из «ждёт доли» на круге сверки балансов для
//! ОСТАНОВЛЕННЫХ сделок (своей задачи у них нет; в том числе после перезапуска): доля тем же распределением
//! (credit_split.rs) по балансу монеты, который уже прочитал этот круг сверки (reconcile_key::total — без отдельного
//! чтения на каждую сделку). Сохранение — только если запись сделки в учёте не менялась с начала (тот же шаг «остановлена»
//! и то же время последнего события): действие человека (кнопка, «Беру на себя») не перетирается. Доля 0 — ничего
//! (решит человек кнопкой). Удержано больше ожидаемого (over) — НЕ подтверждается (круг видит только торговый счёт,
//! монеты на счёте перевода — нет; решит факт всех счетов перед продажей), но журнал core.credit_fact и Telegram
//! deal.credit_hold с цифрами — не молча.
//! Кто вызывает: core/reconcile.rs (на каждую монету биржи, сверенную в круге).
//! От чего зависит: credit_apply.rs (input), credit_split.rs, credit_text.rs, credit_alert.rs (notice), credit_fact.rs,
//! remain_others.rs, reconcile_key.rs, ledger.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::credit_fact::Basis;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Что делает: ключи монет (как у сверки, reconcile_key.rs) остановленных ждущих доли сделок на бирже `v`. Ошибок нет.
pub fn keys(core: &Core, v: &VenueId) -> Vec<String> {
    let mut out: Vec<String> = lk(&core.inner.deals)
        .values()
        .filter(|d| &d.at == v && d.step == Step::Halted && super::credit_fact::pending(d))
        .map(|d| super::reconcile_key::key(core, &d.direction.asset, v))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Что делает: подтверждение долей остановленных ждущих сделок монеты `key` на бирже `v` по балансу круга `fact`
/// (шапка). Принимает: ядро, биржу, ключ монеты сверки, баланс. Ошибок нет.
pub async fn confirm(core: &Core, v: &VenueId, key: &str, fact: Decimal) {
    let list: Vec<Deal> = lk(&core.inner.deals)
        .values()
        .filter(|d| &d.at == v && d.step == Step::Halted && super::credit_fact::pending(d))
        .filter(|d| super::reconcile_key::key(core, &d.direction.asset, v) == key)
        .cloned()
        .collect();
    for mut d in list {
        let o = super::remain_others::on_account(core, &d, v);
        let sp = super::credit_split::split(&super::credit_apply::input(core, &d, fact, &o));
        let share = sp.share_of(&d.id).unwrap_or_default();
        if share <= Decimal::ZERO {
            continue;
        }
        let record = d.qty;
        let text = super::credit_text::describe(core, &d, fact, &o, &sp, (record, share));
        let deps = &core.inner.deps;
        // Удержано больше ожидаемого — круг сверки видит только торговый счёт (balances), монеты на счёте перевода
        // (фонд) он не видит: долю не подтверждаем (решит факт перед продажей — sell_fact.rs), но не молча.
        if sp.over {
            let t = format!(
                "{text} — на круге сверки не подтверждено: удержано больше ожидаемого, доля — по факту \
                             всех счетов перед продажей"
            );
            deps.log(Level::Warn, "core.credit_fact", Some(&v.0), &t, None)
                .await;
            if let Some(tg) = super::credit_alert::notice(&d, record, share) {
                deps.notify("deal.credit_hold", Some(&v.0), &tg).await;
            }
            continue;
        }
        let seen = d.updated_at;
        super::credit_fact::confirm(&mut d, share, Basis::Balance);
        d.note(format!("{text} (подтверждено на круге сверки балансов)"));
        // Сверка шага и времени под замком учёта: человек мог нажать кнопку, пока круг читал баланс.
        let same = lk(&core.inner.deals)
            .get(&d.id)
            .is_some_and(|x| x.step == Step::Halted && x.updated_at == seen);
        if !same {
            continue;
        }
        super::ledger::save(core, &d).await;
        deps.log(Level::Info, "core.credit_fact", Some(&v.0), &text, None)
            .await;
    }
}
