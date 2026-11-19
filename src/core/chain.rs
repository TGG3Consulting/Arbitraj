//! Файл: core/chain.rs
//! Что делает: сети ядра (решение 416 — шаг 1; решение 430 — шаг 2 «много DEX», Тигран 19.11.2026: один бот, две сети,
//! по одной площадке DEX на сеть). Сеть — только из настроек площадки DEX (dex.chain); ядро конкретных сетей не знает.
//! **Сеть по умолчанию** (решение 430): у всех площадок DEX одна сеть — она; сетей несколько — dex.default_chain (вся
//! система). Монета сети по умолчанию — всегда `chain: None` (канонический вид, функция tag): записи учёта до решения 416
//! и все ключи BSC (замки, учёт, настройки монет, пары) остаются прежними — переносить базу не нужно. Монета другой сети —
//! `chain: Some(id)`. Площадка DEX сети — dex_of (одна площадка на сеть). Одна площадка DEX — ответы без чтения
//! настроек, как до решения 430 (поведение BSC прежнее).
//! Кто вызывает: core/coin_account.rs, core/gas_ticker.rs, core/native.rs, core/balances_cache.rs, core/usdt_sources.rs,
//! core/drawdown_fact.rs, core/day_pair.rs, core/subscribe.rs, core/registry_build.rs, core/rebuild*.rs, core/transfers.rs,
//! core/dirctx.rs, core/transfer_tax.rs, report/*, api/wallets_dex.rs; тесты tests/core_chain_416.rs, core_chain_430.rs.
//! От чего зависит: settings (store, read), core/types (Asset, ChainId, VenueId), engine.rs (виды площадок).
//! Настройки: dex.chain (по площадке DEX), dex.default_chain (вся система; только при нескольких сетях).

use crate::core::types::{Asset, ChainId, Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::{Core, rd};

/// Имя настройки сети по умолчанию (решение 430).
pub const DEFAULT: &str = "dex.default_chain";

/// Что делает: сеть площадки DEX (dex.chain). Принимает: хранилище, площадку. Возвращает: id сети.
/// Ошибки: текст — не задана или пуста.
pub fn chain_at(s: &Store, v: &VenueId) -> Result<ChainId, String> {
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
    Ok(ChainId(id))
}

/// Что делает: сеть по умолчанию (монеты без сети) по площадкам DEX: одна сеть у всех — она; разные — dex.default_chain,
/// если это сеть одной из площадок. Принимает: хранилище, площадки DEX. Возвращает: id сети.
/// Ошибки: текст — площадок DEX нет; dex.chain не задана; площадки в разных сетях, а dex.default_chain не задана или
/// не сеть ни одной площадки DEX.
pub fn dex_chain(s: &Store, dex: &[VenueId]) -> Result<ChainId, String> {
    let mut found: Vec<(VenueId, ChainId)> = vec![];
    for v in dex {
        found.push((v.clone(), chain_at(s, v)?));
    }
    let first = match found.first() {
        None => return Err("площадок DEX нет — сеть монеты неизвестна".into()),
        Some((_, c)) => c.clone(),
    };
    if found.iter().all(|(_, x)| *x == first) {
        return Ok(first);
    }
    match s.text(DEFAULT, At::default()) {
        Ok(d) if found.iter().any(|(_, x)| x.0 == d) => Ok(ChainId(d)),
        Ok(d) => Err(format!(
            "{DEFAULT} = «{d}» — нет площадки DEX этой сети ({found:?})"
        )),
        Err(_) => Err(format!(
            "площадки DEX в разных сетях ({found:?}) — сеть монеты без сети не выбрать: задайте {DEFAULT}"
        )),
    }
}

/// Что делает: сеть монеты — своя (Asset.chain) или сеть по умолчанию (dex_chain). Принимает: хранилище, площадки DEX,
/// монету. Возвращает: id сети. Ошибки: как у dex_chain.
pub fn of(s: &Store, dex: &[VenueId], a: &Asset) -> Result<ChainId, String> {
    match &a.chain {
        Some(c) => Ok(c.clone()),
        None => dex_chain(s, dex),
    }
}

/// Что делает: канонический вид сети монеты (решение 430): сеть по умолчанию — None, иначе Some(сеть).
/// Принимает: хранилище, площадки DEX, сеть. Возвращает: значение для Asset.chain. Ошибки: как у dex_chain.
pub fn tag(s: &Store, dex: &[VenueId], c: &ChainId) -> Result<Option<ChainId>, String> {
    let d = dex_chain(s, dex)?;
    if *c == d {
        Ok(None)
    } else {
        Ok(Some(c.clone()))
    }
}

/// Что делает: площадки DEX реестра ядра (по порядку реестра). Принимает: ядро. Ошибок нет.
pub fn dexes(core: &Core) -> Vec<VenueId> {
    rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| v.clone())
        .collect()
}

/// Что делает: площадка DEX сети монеты (сеть `chain` в каноническом виде: None — сеть по умолчанию). Одна площадка DEX
/// — она, без чтения настроек (как до решения 430). Принимает: хранилище, площадки DEX, сеть монеты.
/// Возвращает: площадку. Ошибки: текст — площадок DEX нет; сети нет ни у одной; у сети две площадки DEX.
pub fn dex_of(s: &Store, dex: &[VenueId], chain: Option<&ChainId>) -> Result<VenueId, String> {
    if let [one] = dex
        && chain.is_none()
    {
        return Ok(one.clone());
    }
    let want = match chain {
        Some(c) => c.clone(),
        None => dex_chain(s, dex)?,
    };
    let mut hit = dex
        .iter()
        .filter(|v| chain_at(s, v).is_ok_and(|c| c == want));
    match (hit.next(), hit.next()) {
        (Some(v), None) => Ok(v.clone()),
        (None, _) => Err(format!("нет площадки DEX сети {want} (dex.chain)")),
        (Some(a), Some(b)) => Err(format!(
            "у сети {want} две площадки DEX ({}, {}) — одна площадка DEX на сеть",
            a.0, b.0
        )),
    }
}

/// Что делает: сеть монеты по площадкам DEX ядра. Принимает: ядро, монету. Возвращает: id сети. Ошибки: как у dex_chain.
pub fn of_core(core: &Core, a: &Asset) -> Result<ChainId, String> {
    of(&core.inner.deps.settings, &dexes(core), a)
}

/// Что делает: площадка DEX сети монеты (dex_of). Принимает: ядро, монету. Ошибки: как у dex_of.
pub fn dex_of_asset(core: &Core, a: &Asset) -> Result<VenueId, String> {
    dex_of(&core.inner.deps.settings, &dexes(core), a.chain.as_ref())
}

/// Что делает: площадка DEX сети по умолчанию — замена «первой DEX» (бывший Core::dex_id): кошельки, пара «DEX ↔ DEX»
/// сети по умолчанию. Принимает: ядро. Возвращает: площадку; None — DEX нет или сеть по умолчанию не выбрать.
/// Ошибок нет.
pub fn default_dex(core: &Core) -> Option<VenueId> {
    dex_of(&core.inner.deps.settings, &dexes(core), None).ok()
}

/// Что делает: сеть площадки DEX в каноническом виде (None — сеть по умолчанию). Одна площадка DEX — None без чтения
/// настроек. Принимает: ядро, площадку DEX. Ошибки: как у chain_at / dex_chain.
pub fn tag_of_dex(core: &Core, v: &VenueId) -> Result<Option<ChainId>, String> {
    let dex = dexes(core);
    if dex.len() == 1 && dex[0] == *v {
        return Ok(None);
    }
    let s = &core.inner.deps.settings;
    tag(s, &dex, &chain_at(s, v)?)
}

/// Что делает: сеть направления (решение 430) — сеть его монеты: биржа ↔ DEX — сеть DEX, биржа ↔ биржа — сеть перевода.
/// Принимает: ядро, направление. Возвращает: id сети. Ошибки: как у dex_chain.
pub fn of_dir(core: &Core, d: &Direction) -> Result<ChainId, String> {
    of_core(core, &d.asset)
}

/// Что делает: виды площадок реестра для сети `tag` (решение 430): все биржи и площадки DEX только этой сети (налог
/// перевода монеты — по проверкам DEX её сети). Одна площадка DEX и сеть по умолчанию — реестр как есть, без чтения
/// настроек. Принимает: ядро, сеть (канонический вид). Возвращает: (площадка, вид). Ошибок нет: DEX, чью сеть не
/// прочитать, не входит.
pub fn kinds_in(core: &Core, tag: Option<&ChainId>) -> Vec<(VenueId, VenueKind)> {
    let kinds = rd(&core.inner.kinds).clone();
    let dex = kinds.iter().filter(|(_, k)| *k == VenueKind::Dex).count();
    if dex == 1 && tag.is_none() {
        return kinds;
    }
    kinds
        .into_iter()
        .filter(|(v, k)| {
            *k != VenueKind::Dex || tag_of_dex(core, v).is_ok_and(|t| t.as_ref() == tag)
        })
        .collect()
}
