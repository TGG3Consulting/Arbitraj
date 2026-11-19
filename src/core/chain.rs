//! Файл: core/chain.rs
//! Что делает: решение 416 (шаг 1 двух сетей, Тигран 19.11.2026) — сеть монеты для ядра. Сеть — только из настроек
//! площадки DEX (dex.chain); ядро конкретных сетей не знает. В шаге 1 рабочий код ещё не ставит сеть в монету
//! (Asset.chain = None у всех монет, равенство и ключи — как раньше); записи учёта, сделанные до решения 416 (JSON
//! без поля «chain»), тоже читаются с None. Монета без сети принадлежит сети ЕДИНСТВЕННОЙ площадки DEX — её даёт `of`.
//! Площадок DEX с разными сетями несколько — сети по умолчанию нет (Err): это шаг 2 плана (docs/TASKS.md), и тогда
//! старые записи переносятся явно (как core/coin_scope_migrate.rs решения 211), а не угадываются.
//! Кто вызывает: тесты решения 416 (tests/core_chain_416.rs); шаг 2 — ядро «много DEX» (docs/TASKS.md).
//! От чего зависит: settings (store, read), core/types (Asset, ChainId, VenueId), engine.rs (виды площадок).
//! Настройки: dex.chain (по площадке DEX).

use crate::core::types::{Asset, ChainId, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::{Core, rd};

/// Что делает: сеть единственной сети площадок DEX. Принимает: хранилище, площадки DEX. Возвращает: id сети.
/// Ошибки: текст — площадок DEX нет; dex.chain не задана у площадки; площадки DEX дают разные сети (сети по
/// умолчанию нет — шаг 2).
pub fn dex_chain(s: &Store, dex: &[VenueId]) -> Result<ChainId, String> {
    let mut found: Vec<(VenueId, ChainId)> = vec![];
    for v in dex {
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        let id = s
            .text("dex.chain", at)
            .map_err(|e| format!("{}: {}", v.0, e.message))?;
        if id.trim().is_empty() {
            return Err(format!("{}: dex.chain пуста — задайте id сети", v.0));
        }
        found.push((v.clone(), ChainId(id)));
    }
    match found.first() {
        None => Err("площадок DEX нет — сеть монеты неизвестна".into()),
        Some((_, c)) if found.iter().all(|(_, x)| x == c) => Ok(c.clone()),
        Some(_) => Err(format!(
            "площадки DEX в разных сетях ({found:?}) — сеть монеты без сети не выбрать (шаг 2, docs/TASKS.md)"
        )),
    }
}

/// Что делает: сеть монеты — своя (Asset.chain) или, если её нет (монета биржи, запись до решения 416), сеть площадок
/// DEX (dex_chain). Принимает: хранилище, площадки DEX, монету. Возвращает: id сети. Ошибки: как у dex_chain.
pub fn of(s: &Store, dex: &[VenueId], a: &Asset) -> Result<ChainId, String> {
    match &a.chain {
        Some(c) => Ok(c.clone()),
        None => dex_chain(s, dex),
    }
}

/// Что делает: сеть монеты по площадкам DEX ядра (виды площадок реестра). Принимает: ядро, монету.
/// Возвращает: id сети. Ошибки: как у dex_chain.
pub fn of_core(core: &Core, a: &Asset) -> Result<ChainId, String> {
    let dex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| v.clone())
        .collect();
    of(&core.inner.deps.settings, &dex, a)
}
