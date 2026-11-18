//! Файл: core/credit_base.rs
//! Что делает: решение 413 (t198, кейс 6) — «посторонние» монеты на счёте биржи-получателя ПЕРЕД выводом монеты сделки
//! туда: баланс счёта (торговый + перекладываемые на торговый, free + locked — remain_fact.rs fact_at) минус учёт
//! сделок этой монеты на счёте (подтверждённые — их количество; ждущие доли — запись × (1 − ожидаемое удержание));
//! не меньше 0. Это ручные, старые, пыль закрытых сделок — их распределение долей (credit_split.rs) не отдаёт ни одной
//! сделке.
//!
//! Когда снимок верен: чтение начинается в начале перевода (transfers.rs go) параллельно его подготовке, а вывод ЖДЁТ
//! конца чтения (ready) — значит в снимке монет этого перевода нет (их ещё не отправили). Ждать приходится только
//! «подозрительным» монетам (deposit_hold.rs suspect: ожидаемое удержание или неизмеренный налог) — только у них доля
//! считается по факту; монеты без налога зачисляются по записи, снимок им не нужен (вывод не ждёт ничего). Учёт сделок
//! за время чтения сдвинулся (продажа, зачисление) или баланс не прочитан — снимка нет (None): распределение считает
//! посторонних 0, а замер удержания — неоднозначным. Только в памяти: перезапуск посреди перевода — снимка нет (сделка
//! после перезапуска и так остановлена, решает человек).
//! Кто вызывает: core/transfers.rs (start, ready — перевод монеты сделки на биржу), core/credit_fact.rs (take).
//! От чего зависит: remain_fact.rs (fact_at), remain_others.rs (учёт других), credit_lock.rs, deposit_hold.rs,
//! trade_ops.rs, engine.rs.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::VenueKind;

use super::engine::{Core, lk};

/// Снимки: ID сделки → посторонние (None — снимка нет).
static SLOTS: LazyLock<Mutex<HashMap<String, Option<Decimal>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Ячейки снимков. Ошибок нет.
fn slots() -> std::sync::MutexGuard<'static, HashMap<String, Option<Decimal>>> {
    SLOTS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Чтение снимка, начатое переводом.
pub type Reading = tokio::task::JoinHandle<Option<Decimal>>;

/// Что делает: начинает снимок перед выводом монеты сделки `deal` на биржу `to` — только биржа-получатель и
/// «подозрительная» монета (шапка). Принимает: ядро, ID сделки, получателя, монету. Возвращает: чтение (None — снимок
/// не нужен). Ошибок нет.
pub fn start(core: &Core, deal: &str, to: &VenueId, a: &Asset) -> Option<Reading> {
    if super::trade_ops::kind(core, to) != Ok(VenueKind::Cex)
        || !super::deposit_hold::suspect(core, to, a)
    {
        return None;
    }
    let (c, id, to, a) = (core.clone(), deal.to_string(), to.clone(), a.clone());
    Some(tokio::spawn(async move { read(&c, &id, &to, &a).await }))
}

/// Что делает: дождаться конца чтения (вызывает перевод ПЕРЕД выводом) и запомнить снимок для зачисления.
/// Принимает: ID сделки, чтение. Ошибок нет: задача чтения упала — снимка нет.
pub async fn ready(deal: &str, r: Option<Reading>) {
    if let Some(h) = r {
        let v = h.await.ok().flatten();
        slots().insert(deal.to_string(), v);
    }
}

/// Что делает: забирает снимок (после зачисления — credit_fact::on_credit). Возвращает: посторонние или None. Ошибок нет.
pub fn take(deal: &str) -> Option<Decimal> {
    slots().remove(deal).flatten()
}

/// Что делает: посторонние = max(0, баланс − учёт сделок монеты на счёте), если учёт не сдвинулся за чтение.
/// Принимает: ядро, ID сделки, получателя, монету. Возвращает: посторонние или None. Ошибок нет.
async fn read(core: &Core, deal: &str, to: &VenueId, a: &Asset) -> Option<Decimal> {
    let own = lk(&core.inner.deals).get(deal).cloned()?;
    let _g = super::credit_lock::fact(to, a).await;
    let before = super::remain_others::on_account(core, &own, to);
    let fact = super::remain_fact::fact_at(core, &own, to).await.ok()?;
    let after = super::remain_others::on_account(core, &own, to);
    if before != after {
        return None;
    }
    let keep = Decimal::ONE - super::deposit_hold::expected_pct(core, to, a) / Decimal::ONE_HUNDRED;
    let pending: Decimal = after.pending.iter().map(|p| p.record * keep).sum();
    Some((fact - after.qty - pending).max(Decimal::ZERO))
}
