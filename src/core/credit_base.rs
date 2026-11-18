//! Файл: core/credit_base.rs
//! Что делает: решение 413 (t198, кейс 6) — «посторонние» монеты на счёте биржи-получателя перед зачислением перевода
//! монеты сделки: баланс счёта (торговый + перекладываемые на торговый, free + locked — remain_fact.rs fact_at) минус
//! учёт сделок этой монеты на счёте (подтверждённые — их количество; ждущие доли — запись × (1 − ожидаемое удержание));
//! не меньше 0. Это ручные, старые, пыль закрытых сделок — их распределение долей (credit_split.rs) не отдаёт ни одной
//! сделке.
//!
//! Ревью t198 (С1): снимок снимается ОДНОВРЕМЕННО с выводом (вывод его не ждёт): депозит биржи возможен только после
//! того, как транзакция вывода попала в блок и набрала подтверждения биржи, — чтение баланса, начатое до запроса вывода,
//! монет этого перевода не видит. Не успело к зачислению, упало или учёт сделок за чтение сдвинулся — снимка нет
//! (правило Б3: доля не выше запись × (1 − ожидаемое удержание)).
//! Ревью t198 (Б1): через счёт идёт сделка в пути (шаг «перевод») — её монеты могли уже прийти, а записи депозита ещё
//! нет: такой снимок неоднозначен — не пишется (None), «посторонними» её монеты не становятся никогда.
//! Ревью t198: снимок привязан к переводу — к хешу транзакции вывода (bind при зачислении), а не к сделке и бирже;
//! сорванный перевод снимает свою ячейку (Drop у Snap). Сохранение — в учёте сделки (Deal.credit.outsider, SQLite):
//! перезапуск его не теряет.
//! Кто вызывает: core/transfers.rs (start — перевод монеты сделки на биржу, bind — после зачисления),
//! core/credit_fact.rs (take).
//! От чего зависит: remain_fact.rs (fact_at), remain_others.rs (учёт других), credit_lock.rs, deposit_hold.rs,
//! trade_ops.rs, native.rs, gateway/tx_hash.rs, engine.rs.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::VenueKind;

use super::engine::{Core, lk};

/// Чтения: номер перевода → итог (None — ещё читается; Some(None) — снимка нет; Some(Some(v)) — посторонние).
static READS: LazyLock<Mutex<HashMap<u64, Option<Option<Decimal>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Снимки, привязанные к хешу вывода (нижний регистр, без 0x) — до записи зачисления в сделку.
static BOUND: LazyLock<Mutex<HashMap<String, Decimal>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Номера переводов.
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Снимок одного перевода. Сорванный перевод (Snap выброшен без bind) снимает свою ячейку.
pub struct Snap {
    key: u64,
}

impl Drop for Snap {
    fn drop(&mut self) {
        READS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
    }
}

/// Хеш в ключ (как gateway::tx_hash::same: нижний регистр, без 0x, до «:»). Ошибок нет.
fn norm(tx: &str) -> String {
    let t = tx.split(':').next().unwrap_or(tx).to_ascii_lowercase();
    t.trim_start_matches("0x").to_string()
}

/// Что делает: начинает снимок посторонних перед выводом монеты сделки `deal` на биржу `to` (DEX-получатель, нативная
/// монета — нет). Вывод не ждёт (шапка). Принимает: ядро, ID сделки, получателя, монету. Возвращает: снимок перевода
/// (None — не нужен). Ошибок нет.
pub fn start(core: &Core, deal: &str, to: &VenueId, a: &Asset) -> Option<Snap> {
    if super::trade_ops::kind(core, to) != Ok(VenueKind::Cex) || super::native::is_native(core, a) {
        return None;
    }
    let key = NEXT.fetch_add(1, Ordering::Relaxed);
    READS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, None);
    let (c, id, to, a) = (core.clone(), deal.to_string(), to.clone(), a.clone());
    tokio::spawn(async move {
        let v = Box::pin(read(&c, &id, &to, &a)).await;
        let mut m = READS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(slot) = m.get_mut(&key) {
            *slot = Some(v);
        }
    });
    Some(Snap { key })
}

/// Что делает: зачисление получено — снимок (если чтение кончилось) привязывается к хешу вывода; не кончилось или
/// снимка нет — ничего (правило Б3). Принимает: снимок перевода, хеш вывода. Ошибок нет.
pub fn bind(snap: Option<Snap>, tx: Option<&str>) {
    let (Some(s), Some(tx)) = (snap, tx) else {
        return;
    };
    let v = READS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&s.key)
        .copied()
        .flatten()
        .flatten();
    if let Some(v) = v {
        BOUND
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(norm(tx), v);
    }
}

/// Что делает: забирает снимок по хешу вывода (credit_fact::on_credit). Возвращает: посторонние или None. Ошибок нет.
pub fn take(tx: Option<&str>) -> Option<Decimal> {
    let tx = tx?;
    BOUND
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&norm(tx))
}

/// Что делает: посторонние по учёту других (чистая функция): сделка в пути через счёт — None (ревью Б1); иначе
/// max(0, факт − подтверждённые − ждущие × (1 − ожидаемое удержание)). Принимает: факт, других, ожидаемое удержание %.
/// Возвращает: посторонние или None. Ошибок нет.
pub fn outsider_of(
    fact: Decimal,
    o: &super::remain_others::Others,
    hold_pct: Decimal,
) -> Option<Decimal> {
    if !o.unclear.is_empty() || o.busy {
        return None;
    }
    let keep = Decimal::ONE - hold_pct / Decimal::ONE_HUNDRED;
    let pending: Decimal = o.pending.iter().map(|p| p.record * keep).sum();
    Some((fact - o.qty - pending).max(Decimal::ZERO))
}

/// Что делает: чтение снимка: учёт других до и после чтения баланса одинаков — outsider_of, иначе None.
/// Принимает: ядро, ID сделки, получателя, монету. Возвращает: посторонние или None. Ошибок нет.
async fn read(core: &Core, deal: &str, to: &VenueId, a: &Asset) -> Option<Decimal> {
    let own = lk(&core.inner.deals).get(deal).cloned()?;
    let g = super::credit_lock::fact(core, to, a).await;
    let before = super::remain_others::on_account(core, &own, to);
    let fact = Box::pin(super::remain_fact::fact_at(core, &own, to))
        .await
        .ok()?;
    let mut after = super::remain_others::on_account(core, &own, to);
    after.busy = g.is_none();
    if before.qty != after.qty || before.pending != after.pending || before.unclear != after.unclear
    {
        return None;
    }
    outsider_of(fact, &after, super::deposit_hold::expected_pct(core, to, a))
}
