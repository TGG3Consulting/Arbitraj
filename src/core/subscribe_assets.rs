//! Файл: core/subscribe_assets.rs
//! Что делает: добавочные монеты подписки площадки (вынесено из core/subscribe.rs, решение 431 — файл вырос сверх
//! ~200 строк, TASKS 430 л): монеты venue.price_assets биржи (решение 181), котируемая монета площадки DEX (USDT её сети),
//! промежуточные монеты DEX (решение 52; тикер — по записи справочника сети площадки DEX, решение 431) и один стакан на
//! тикер у биржи для монет другой сети (решение 431).
//! Кто вызывает: core/subscribe.rs (wanted).
//! От чего зависит: engine.rs, chain.rs, intermediate.rs, registry_chains.rs, settings.
//! Настройки: venue.price_assets [биржа], venue.quote_asset [DEX], dex.intermediate_* — через intermediate.rs.

use crate::core::types::{Asset, ChainId, VenueId};

use super::engine::{Core, rd};

/// Решение 181: монеты из venue.price_assets биржи — только цена (стакан), без адреса контракта, как BNB газа.
/// Не задана — ничего (как раньше). Ошибок нет.
pub(super) fn price_assets(core: &Core, venue: &VenueId) -> Vec<Asset> {
    let at = crate::settings::read::At {
        venue: Some(&venue.0),
        coin: None,
    };
    core.inner
        .deps
        .settings
        .list("venue.price_assets", at)
        .unwrap_or_default()
        .into_iter()
        .map(|t| Asset {
            chain: None,
            ticker: t.trim().to_uppercase(),
            contract: None,
        })
        .filter(|a| !a.ticker.is_empty())
        .collect()
}

/// Котируемая монета площадки DEX: тикер — venue.quote_asset этой площадки, адрес — USDT в BSC из справочника бирж.
/// None — тикер не задан или адрес USDT ещё неизвестен (причина видна на экране «Монеты» и в журнале). Ошибок нет.
pub(super) fn dex_quote(core: &Core, venue: &VenueId) -> Option<Asset> {
    let at = crate::settings::read::At {
        venue: Some(&venue.0),
        coin: None,
    };
    let ticker = core
        .inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .ok()?;
    // Решение 430: USDT сети этой площадки DEX (сеть по умолчанию — адрес из справочника бирж, как раньше).
    let tag = super::chain::tag_of_dex(core, venue).ok()?;
    let addr = if tag.as_ref().is_some_and(|c| c.as_str() == "base") && ticker == "USDC" {
        let registry = super::engine::rd(&core.inner.networks);
        let known = registry.values().any(|rows| rows.iter().any(|row| row.asset.chain == tag
            && row.asset.ticker == "USDC" && row.asset.contract == Some(super::settlement::BASE_USDC)));
        if !known { return None; }
        super::settlement::BASE_USDC
    } else {
        if tag.as_ref().is_some_and(|c| c.as_str() == "base") && ticker != "USDT" {
            return None;
        }
        super::registry_chains::usdt_of(core, tag.as_ref()).ok()?
    };
    Some(Asset {
        chain: tag,
        ticker,
        contract: Some(addr),
    })
}

/// Промежуточные монеты площадок DEX, которые биржи знают по адресу (тикер для стакана). Принимает: ядро, биржу
/// (Some — тикер этой биржи; None — тикер любой биржи, для DEX), площадку DEX (Some — только её список и её сеть,
/// решение 430; None — все площадки DEX). Ошибок нет.
pub(super) fn intermediate_assets(
    core: &Core,
    venue: Option<&VenueId>,
    only: Option<&VenueId>,
) -> Vec<Asset> {
    let s = &core.inner.deps.settings;
    let nets = rd(&core.inner.networks);
    // Решение 211: бирже — тикер промежуточной монеты в ЕЁ справочнике сетей (не знает адреса — не подписывается);
    // DEX — тикер любой биржи (у DEX тикер только показной).
    // Решение 431: тикер — по записи справочника сети площадки DEX (сеть и адрес), а не по одному адресу.
    let name = |a, chain: Option<&ChainId>| match venue {
        Some(v) => nets.get(v).and_then(|l| {
            l.iter()
                .find(|i| i.asset.contract == Some(a) && i.asset.chain.as_ref() == chain)
                .map(|i| i.asset.ticker.clone())
        }),
        None => super::intermediate::ticker_of_in(&nets, chain, a),
    };
    let mut out = vec![];
    // Список площадок DEX — копией: сеть площадки (chain.rs) сама читает реестр, блокировка не держится вложенно.
    for v in super::chain::dexes(core) {
        if only.is_some_and(|o| *o != v) {
            continue;
        }
        // Решение 430: у DEX монета промежуточной — в сети площадки (канонический вид; не прочитать — пропуск); у
        // биржи — без сети (монета биржи, стакан по тикеру).
        let (tag, dex_tag) = match (venue, super::chain::tag_of_dex(core, &v)) {
            (_, Err(_)) => continue,
            (Some(_), Ok(t)) => (None, t),
            (None, Ok(t)) => (t.clone(), t),
        };
        if let super::intermediate::Intermediate::On(list) = super::intermediate::read(s, &v) {
            for a in list {
                if let Some(t) = name(a, dex_tag.as_ref()) {
                    out.push(Asset {
                        chain: tag.clone(),
                        ticker: t,
                        contract: Some(a),
                    });
                }
            }
        }
    }
    out
}

/// Что делает: решение 431 (TASKS 430 и) — бирже один стакан на тикер: монета другой сети (Asset.chain) с тикером, который
/// уже есть у монеты без сети или у другой монеты с сетью, отдельной записью не идёт (у биржи стакан — по тикеру, сеть —
/// только у ввода/вывода). Монет с сетью нет — список не меняется (BSC как раньше). Принимает: список. Ошибок нет.
pub(super) fn one_book(out: &mut Vec<Asset>) {
    if out.iter().all(|a| a.chain.is_none()) {
        return;
    }
    let mut seen: std::collections::HashSet<String> = out
        .iter()
        .filter(|a| a.chain.is_none())
        .map(|a| a.ticker.clone())
        .collect();
    out.retain(|a| a.chain.is_none() || seen.insert(a.ticker.clone()));
}
