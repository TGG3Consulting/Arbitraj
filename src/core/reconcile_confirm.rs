//! Файл: core/reconcile_confirm.rs
//! Что делает: защита сверки балансов от ложной «Паузы» (решение 112). Случай 02.11 18:16: «ATOM по учёту 28.86» =
//! 15.06 + 13.80 двух сделок, которые за время сверки уже продали монеты и закрылись. Учёт («сколько монет бота на
//! площадке») снимался в начале круга, а балансы площадок приходили позже (тогда — минутами: шторм нагрузки MEXC),
//! и сравнивался старый учёт со свежим балансом. Теперь:
//! (1) учёт площадки снимается до и после запроса её балансов; монета, по которой за это время сделка сменила шаг,
//!     площадку, объём или записала событие, в этом круге не сверяется (сравнивать не с чем);
//! (2) расхождение по (площадке, монете) ставит паузу и уходит в уведомление, только если держится
//!     ledger.reconcile_confirm_rounds кругов подряд (не задано или 0 — с первого круга, как раньше).
//! Решение 118 (Я-11): «балансы площадки не получены» — пауза этой площадки (pause_venue), а не системы.
//! Кто вызывает: core/reconcile.rs.
//! От чего зависит: engine.rs (сделки), deal.rs.
//! Настройки: ledger.reconcile_confirm_rounds.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::settings::read::At;

use super::deal::Step;
use super::engine::{Core, lk};

/// Отпечаток сделки для сравнения «до» и «после» запроса балансов: ID, шаг, объём, время последнего события.
type Print = (String, Step, Decimal, DateTime<Utc>);

/// Учёт площадки: ключ монеты (решение 211: тикер биржи; у DEX — адрес «0x…», reconcile_key.rs) → (сколько монет бота
/// там должно быть, отпечатки сделок).
pub type Expected = HashMap<String, (Decimal, Vec<Print>)>;

/// Учёт площадки сейчас: открытые сделки не в пути (не покупка и не перевод), монеты которых лежат на площадке.
/// Принимает: ядро, площадку. Возвращает: Expected. Ошибок нет.
pub fn expected(
    core: &Core,
    venue: &VenueId,
    acc: Option<&crate::core::types::DexAccountId>,
) -> Expected {
    let mut out: Expected = HashMap::new();
    let deals: Vec<_> = lk(&core.inner.deals)
        .values()
        .filter(|d| d.is_open() && d.step != Step::Transfer && d.step != Step::Buying)
        .filter(|d| &d.at == venue)
        .cloned()
        .collect();
    for d in deals {
        // Решение 132: у DEX — только сделки этого кошелька.
        if acc.is_some()
            && super::dex_account::of_deal(core, &d.id)
                .ok()
                .flatten()
                .as_ref()
                != acc
        {
            continue;
        }
        // Решение 211: биржа — по её тикеру монеты; кошелёк DEX — по адресу (ревью В2, reconcile_key.rs).
        let t = super::reconcile_key::key(core, &d.direction.asset, venue);
        let e = out.entry(t).or_default();
        e.0 += d.qty;
        e.1.push((d.id.clone(), d.step, d.qty, d.updated_at));
    }
    for e in out.values_mut() {
        e.1.sort_by(|a, b| a.0.cmp(&b.0));
    }
    out
}

/// Монеты, учёт которых не менялся за время запроса балансов (их можно сверять), с числом «по учёту».
/// Принимает: учёт до и после. Возвращает: монета → сколько должно быть. Ошибок нет.
pub fn stable(before: &Expected, after: &Expected) -> Vec<(String, Decimal)> {
    after
        .iter()
        .filter(|(t, e)| before.get(*t) == Some(*e))
        .map(|(t, e)| (t.clone(), e.0))
        .collect()
}

/// Счётчик кругов подряд с расхождением по ключу «площадка/монета».
#[derive(Default)]
pub struct Streak {
    rounds: HashMap<String, u64>,
}

impl Streak {
    /// Отмечает круг: ключи с расхождением в этом круге (остальные обнуляются). Возвращает: ключи, где расхождение
    /// держится не меньше ledger.reconcile_confirm_rounds кругов (не задано или 0 — каждый ключ сразу), и для
    /// неподтверждённых — (ключ, кругов сейчас, нужно). Ошибок нет.
    pub fn mark(
        &mut self,
        core: &Core,
        keys: &[String],
    ) -> (HashSet<String>, Vec<(String, u64, u64)>) {
        let need = core
            .inner
            .deps
            .settings
            .integer("ledger.reconcile_confirm_rounds", At::default())
            .unwrap_or(0)
            .max(1);
        self.rounds.retain(|k, _| keys.contains(k));
        let mut ok = HashSet::new();
        let mut wait = vec![];
        for k in keys {
            let n = self.rounds.entry(k.clone()).or_default();
            *n += 1;
            if *n >= need {
                ok.insert(k.clone());
            } else {
                wait.push((k.clone(), *n, need));
            }
        }
        (ok, wait)
    }
}

/// Окончание ключа проблемы «балансы площадки не получены» (решение 118, Я-11).
pub const UNAVAILABLE: &str = "/балансы";

/// Проблема — «балансы площадки не получены» (пауза площадки, не системы). Ошибок нет.
pub fn is_unavailable(f: &super::reconcile::Found) -> bool {
    f.3.as_deref().is_some_and(|k| k.ends_with(UNAVAILABLE))
}

/// Ставит площадку на паузу (risk.venue_paused), если она ещё не на паузе; снятие — кнопкой «Снять паузу», как у
/// паузы после ошибок подряд. Принимает: площадку, причину. Ошибок нет.
pub fn pause_venue(core: &Core, v: VenueId, why: &str) {
    let fresh = {
        let mut r = lk(&core.inner.risk);
        let fresh = !r.venue_paused.contains_key(&v);
        r.venue_paused.entry(v).or_insert_with(|| why.to_string());
        fresh
    };
    if fresh {
        super::risk::publish(core);
    }
}
