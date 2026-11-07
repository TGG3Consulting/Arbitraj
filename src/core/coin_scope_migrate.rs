//! Файл: core/coin_scope_migrate.rs
//! Что делает: решение 211 — перенос настроек области «монета» со старого ключа по тикеру («coin:CAKE») на адрес
//! контракта («coin:0x0e09…»): монета = адрес. Переносится только однозначное: у тикера ровно один адрес среди всех
//! площадок справочника (показной тикер монеты и тикеры каждой площадки). Неоднозначное (несколько адресов) и
//! неизвестное (ни одного) остаётся как есть — журнал с перечнем; ручная отметка coin.blocked по старому тикеру
//! продолжает блокировать монеты с этим тикером (core/blacklist.rs manual), пока человек не задаст её по адресу.
//! Уже есть значение по адресу — его не трогаем, старое по тикеру остаётся (журнал «конфликт»).
//! Запуск: после сборки справочника, когда справочники сетей ВСЕХ включённых бирж получены (иначе неполный справочник
//! мог бы признать неоднозначный тикер однозначным); один раз за процесс; повтор безвреден (переносить нечего).
//! Кто вызывает: core/rebuild.rs.
//! От чего зависит: settings (store, scope), core/registry.rs (CoinEntry), storage/journal.
//! Настройки: все настройки с областью «монета» (coin.blocked, coin.blocked_note, profit.price_move_k,
//! transfer.credit_amount_tolerance_pct, transfer.credit_mismatch_s, transfer.max_time_s …).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use alloy_primitives::Address;

use crate::settings::catalog;
use crate::settings::scope::{Scope, coin_id, is_address_id};
use crate::settings::store::Store;
use crate::storage::journal::Level;

use super::engine::Core;
use super::registry::CoinEntry;

/// Кто меняет настройку — в журнал изменений настроек.
pub const AUTHOR: &str = "миграция решения 211";

/// Перенос выполнен в этом процессе.
static DONE: AtomicBool = AtomicBool::new(false);

/// План переноса: что куда, что оставить и почему.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// (тикер, адрес): все значения области «coin:тикер» → «coin:адрес».
    pub moves: Vec<(String, Address)>,
    /// Тикер — несколько адресов: (тикер, адреса).
    pub ambiguous: Vec<(String, Vec<Address>)>,
    /// Тикера нет ни у одной монеты с адресом.
    pub unknown: Vec<String>,
}

/// Тикер → адреса по справочнику (показной тикер и тикеры площадок; монеты без адреса не участвуют). Ошибок нет.
pub fn addresses_by_ticker(coins: &[CoinEntry]) -> BTreeMap<String, BTreeSet<Address>> {
    let mut m: BTreeMap<String, BTreeSet<Address>> = BTreeMap::new();
    for c in coins {
        let Some(a) = c.asset.bsc_contract else {
            continue;
        };
        m.entry(c.asset.ticker.clone()).or_default().insert(a);
        for p in &c.presence {
            m.entry(p.ticker.clone()).or_default().insert(a);
        }
    }
    m
}

/// Старые id области «монета» (не адреса) среди заданных настроек каталога (значения убранных из каталога настроек
/// не переносятся: задать их нельзя, решение 59). Ошибок нет.
pub fn old_ids(s: &Store) -> BTreeSet<String> {
    s.all()
        .into_iter()
        .filter_map(|(n, sc, _)| match sc {
            Scope::Coin(id) if !is_address_id(&id) && catalog::find(&n).is_some() => Some(id),
            _ => None,
        })
        .collect()
}

/// Что делает: перенос по плану — каждое значение «coin:тикер» → «coin:адрес» (запись в журнал изменений настроек
/// с автором AUTHOR), старое значение очищается. Значение по адресу уже есть — не трогаем (конфликт).
/// Принимает: хранилище, план. Возвращает: (перенесено, конфликты, сбои записи). Ошибок нет: сбой — в списке.
pub async fn apply(s: &Store, p: &Plan) -> (usize, Vec<String>, Vec<String>) {
    let (mut moved, mut conflicts, mut failed) = (0usize, vec![], vec![]);
    for (ticker, addr) in &p.moves {
        let from = Scope::Coin(ticker.clone());
        let to = Scope::Coin(coin_id(ticker, Some(addr)));
        let rows: Vec<(String, serde_json::Value)> = s
            .all()
            .into_iter()
            .filter(|(n, sc, _)| *sc == from && catalog::find(n).is_some())
            .map(|(n, _, v)| (n, v))
            .collect();
        for (name, v) in rows {
            if s.get_exact(&name, &to).is_some() {
                conflicts.push(format!("{name} {ticker}→{addr:#x}"));
                continue;
            }
            let r = async {
                s.set(&name, to.clone(), Some(v), AUTHOR).await?;
                s.set(&name, from.clone(), None, AUTHOR).await
            };
            match r.await {
                Ok(_) => moved += 1,
                Err(e) => failed.push(format!("{name} {ticker}: {e}")),
            }
        }
    }
    (moved, conflicts, failed)
}

/// План по старым id области «монета» (не адресам). Принимает: id областей, тикер → адреса. Ошибок нет.
pub fn plan(old_ids: &BTreeSet<String>, by_ticker: &BTreeMap<String, BTreeSet<Address>>) -> Plan {
    let mut p = Plan::default();
    for t in old_ids.iter().filter(|t| !is_address_id(t)) {
        match by_ticker
            .get(t)
            .map(|s| s.iter().copied().collect::<Vec<_>>())
        {
            Some(v) if v.len() == 1 => p.moves.push((t.clone(), v[0])),
            Some(v) if !v.is_empty() => p.ambiguous.push((t.clone(), v)),
            _ => p.unknown.push(t.clone()),
        }
    }
    p
}

/// Что делает: перенос при пересборке справочника, если справочники сетей всех включённых бирж получены и перенос в
/// этом процессе ещё не сделан. Принимает: ядро, справочник, все ли справочники сетей получены. Ошибок нет: сбой
/// записи — в журнал, перенос повторится при следующей пересборке.
pub async fn run(core: &Core, coins: &[CoinEntry], networks_complete: bool) {
    if !networks_complete || DONE.load(Ordering::Relaxed) {
        return;
    }
    let s = &core.inner.deps.settings;
    let old: BTreeSet<String> = old_ids(s);
    let p = plan(&old, &addresses_by_ticker(coins));
    let (moved, conflicts, failed) = apply(s, &p).await;
    if failed.is_empty() {
        DONE.store(true, Ordering::Relaxed);
    }
    if old.is_empty() {
        return;
    }
    let amb: Vec<String> = p
        .ambiguous
        .iter()
        .map(|(t, v)| {
            let a: Vec<String> = v.iter().map(|x| format!("{x:#x}")).collect();
            format!("{t} ({})", a.join(", "))
        })
        .collect();
    let mut text = format!(
        "Решение 211: настройки монет перенесены с тикера на адрес контракта — значений {moved}; неоднозначных тикеров {}",
        amb.len()
    );
    for (label, list) in [
        (
            "неоднозначные (несколько адресов — задайте по адресу вручную; ручной блок по тикеру действует)",
            &amb,
        ),
        (
            "неизвестные (нет монеты с адресом под таким тикером)",
            &p.unknown,
        ),
        (
            "уже заданы по адресу — старые по тикеру оставлены",
            &conflicts,
        ),
        ("не записаны (повтор при следующей пересборке)", &failed),
    ] {
        if !list.is_empty() {
            text.push_str(&format!("; {label}: {}", list.join(", ")));
        }
    }
    let level = if amb.is_empty() && failed.is_empty() {
        Level::Info
    } else {
        Level::Warn
    };
    core.inner
        .deps
        .log(level, "core.coin_scope_migrate", None, &text, None)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(n: u8) -> Address {
        Address::repeat_byte(n)
    }

    #[test]
    fn plan_moves_only_unambiguous() {
        let mut by: BTreeMap<String, BTreeSet<Address>> = BTreeMap::new();
        by.entry("CAKE".into()).or_default().insert(a(1));
        by.entry("NMT".into()).or_default().insert(a(2));
        by.entry("NETMIND".into()).or_default().insert(a(2));
        by.entry("BOMB".into()).or_default().extend([a(3), a(4)]);
        let addr_id = format!("{:#x}", a(9));
        let old: BTreeSet<String> = ["CAKE", "NETMIND", "BOMB", "GONE", addr_id.as_str()]
            .into_iter()
            .map(String::from)
            .collect();
        let p = plan(&old, &by);
        assert_eq!(
            p.moves,
            vec![("CAKE".into(), a(1)), ("NETMIND".into(), a(2))]
        );
        assert_eq!(p.ambiguous, vec![("BOMB".into(), vec![a(3), a(4)])]);
        assert_eq!(p.unknown, vec!["GONE".to_string()]);
        // Повтор: после переноса старых id нет — план пустой (id адреса не трогается).
        let again: BTreeSet<String> = [addr_id].into_iter().collect();
        assert_eq!(plan(&again, &by), Plan::default());
    }
}
