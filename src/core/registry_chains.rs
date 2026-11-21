//! Файл: core/registry_chains.rs
//! Что делает: решение 430 (шаг 2 двух сетей) — справочник монет по сетям. Площадки DEX делятся на группы по сети
//! (канонический вид core/chain.rs: сеть по умолчанию — None); справочник собирается отдельно для каждой сети: все
//! биржи + площадки DEX этой сети, записи сетей бирж этой сети (Asset.chain записи справочника сетей), свой USDT, своя
//! нативная монета. Монете справочника ставится сеть группы — DEX-присутствие получают только площадки DEX её сети,
//! направления несут сеть в монете. Одна группа (одна площадка DEX или все в одной сети) — входы прежние, без копий
//! справочника сетей (решение 129): поведение BSC как раньше.
//! Решение 489: в группы входят только площадки DEX в игре (core/chain.rs in_play: зарегистрирована и venue.enabled = да
//! или её держат идущие сделки) — то же понятие, что у привязки записей к сети (net_chain.rs); выключенная площадка без
//! сделок группы не даёт. Вызывающие передают реестр (chain::dexes); DEX-присутствие — только у включённых
//! (rebuild_build.rs held_only).
//! USDT сети по умолчанию — Inner.usdt (как раньше), других сетей — Inner.usdt_other (по справочникам бирж этой сети;
//! биржи отдают адреса по сетям — шаг 3 плана).
//! Кто вызывает: core/rebuild_build.rs (сборка), core/rebuild.rs (USDT других сетей), core/subscribe.rs (USDT площадки
//! DEX), core/transfer_tax.rs.
//! От чего зависит: core/chain.rs, registry_build.rs (Networks, usdt_address), engine.rs.
//! Настройки: dex.chain, dex.default_chain, venue.enabled (через chain.rs).

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::Address;

use crate::core::types::{ChainId, VenueId};

use super::engine::{Core, rd};
use super::registry_build::Networks;

/// Группа площадок DEX одной сети (tag — канонический вид сети: None — сеть по умолчанию).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub tag: Option<ChainId>,
    pub dex: Vec<VenueId>,
}

/// Что делает: площадки DEX по сетям. Группа сети по умолчанию есть всегда (первой; у бирж без DEX — пустая): монеты бирж
/// без сети (справочник сетей бирж до шага 3) собираются в ней. Площадка DEX, чью сеть не прочитать (dex.chain,
/// dex.default_chain при двух сетях), ни в одну группу не входит — DEX-присутствия у неё нет (причина — в журнале
/// подписки и в «Проверить подключение»). Одна площадка DEX — группа по умолчанию с ней, без чтения настроек.
/// Решение 489: площадка не в игре (выключена и сделок нет, chain::in_play) ни в одну группу не входит.
/// Принимает: ядро, площадки DEX (по порядку реестра). Возвращает: группы. Ошибок нет.
pub fn groups(core: &Core, dex: &[VenueId]) -> Vec<Group> {
    let mut out = vec![Group {
        tag: None,
        dex: vec![],
    }];
    for v in dex {
        if !super::chain::in_play(core, v) {
            continue;
        }
        let Ok(tag) = super::chain::tag_of_dex(core, v) else {
            continue;
        };
        match out.iter_mut().find(|g| g.tag == tag) {
            Some(g) => g.dex.push(v.clone()),
            None => out.push(Group {
                tag,
                dex: vec![v.clone()],
            }),
        }
    }
    out
}

/// Что делает: справочник сетей бирж только для сети `tag` (запись справочника — Asset.chain). Принимает: справочник,
/// сеть. Возвращает: новый справочник (копии только отфильтрованных списков). Ошибок нет.
pub fn nets_of(nets: &Networks, tag: Option<&ChainId>) -> Networks {
    nets.iter()
        .map(|(v, l)| {
            let f: Vec<_> = l
                .iter()
                .filter(|i| i.asset.chain.as_ref() == tag)
                .cloned()
                .collect();
            (v.clone(), Arc::new(f))
        })
        .collect()
}

/// Что делает: адреса USDT сетей не по умолчанию — из справочников сетей бирж этой сети (registry_build::usdt_address_in;
/// записи сети — по Asset.chain, решение 431).
/// Сеть, по которой биржи адреса не дали, — без записи. Принимает: группы, справочник сетей, котируемые монеты бирж.
/// Возвращает: сеть → адрес. Ошибок нет.
pub fn usdt_other(
    groups: &[Group],
    nets: &Networks,
    quote: &HashMap<VenueId, String>,
) -> HashMap<ChainId, Address> {
    let mut out = HashMap::new();
    for g in groups {
        if let Some(c) = &g.tag
            && let Ok(a) = super::registry_build::usdt_address_in(nets, quote, Some(c))
        {
            out.insert(c.clone(), a);
        }
    }
    out
}

/// Что делает: адрес USDT сети (None — сеть по умолчанию: Inner.usdt, как раньше). Принимает: ядро, сеть.
/// Возвращает: адрес. Ошибки: текст — адрес USDT сети ещё не известен (причина).
pub fn usdt_of(core: &Core, tag: Option<&ChainId>) -> Result<Address, String> {
    match tag {
        None => rd(&core.inner.usdt).clone(),
        Some(c) => rd(&core.inner.usdt_other).get(c).copied().ok_or_else(|| {
            format!("ни одна биржа не сообщила адрес котируемой монеты (USDT) в сети {c}")
        }),
    }
}
