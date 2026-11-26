//! Файл: venues/coinex/own_chains.rs
//! Что делает: решение 431 (шаг 3 двух сетей) — свои сети CoinEx: у каждой два имени — в справочнике ввода/вывода
//! (поле chain: venue.bsc_network, venue.arbitrum_network) и в /v2/assets/info (chain_info.chain_name:
//! venue.bsc_contract_chain, venue.arbitrum_contract_chain), — и разбор справочника с адресами контракта по каждой своей
//! сети (contracts.rs по цепи, networks_parse.rs parse_with). Сеть BSC обязательна (как раньше), Arbitrum — только если
//! заданы оба имени.
//! Кто вызывает: wallet.rs.
//! От чего зависит: contracts.rs, networks_parse.rs, ctx.rs, gateway/own_nets.rs.
//! Настройки: venue.bsc_network, venue.bsc_contract_chain (обязательные), venue.arbitrum_network,
//! venue.arbitrum_contract_chain (необязательные), venue.deposit_min_absent_ok — по площадке coinex.

use std::collections::HashMap;

use alloy_primitives::Address;
use serde_json::Value;

use crate::core::types::VenueId;
use crate::gateway::errors::GatewayResult;
use crate::gateway::own_nets;

use super::ctx::Ctx;
use super::networks_parse::{Parsed, parse_with};

/// Имена цепи в /v2/assets/info для других сетей — в том же порядке, что own_nets::OTHER.
const OTHER_INFO: [&str; own_nets::OTHER.len()] = [
    "venue.arbitrum_contract_chain",
    "venue.base_contract_chain",
    "venue.ethereum_contract_chain",
];

/// Свои сети CoinEx и «минимум депозита неизвестен — 0».
#[derive(Debug, Clone)]
pub struct Names {
    /// (имя сети в справочнике ввода/вывода, имя цепи в /assets/info); первая — BSC.
    pub pairs: Vec<(String, String)>,
    pub min_ok: bool,
}

/// Что делает: читает имена своих сетей. Принимает: окружение. Возвращает: Names. Ошибки: NotConfigured — не задана
/// venue.bsc_network или venue.bsc_contract_chain (как раньше).
pub fn read(ctx: &Ctx) -> GatewayResult<Names> {
    let mut pairs = vec![(
        ctx.text(own_nets::BSC)?,
        ctx.text("venue.bsc_contract_chain")?,
    )];
    for (net, info) in own_nets::OTHER.iter().zip(OTHER_INFO) {
        if let (Ok(n), Ok(c)) = (ctx.text(net), ctx.text(info))
            && !n.is_empty()
            && !c.is_empty()
        {
            pairs.push((n, c));
        }
    }
    Ok(Names {
        pairs,
        min_ok: ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false),
    })
}

/// Что делает: адреса по своим сетям из /assets/info и разбор справочника ввода/вывода. Принимает: data /assets/info,
/// data справочника ввода/вывода, имена, площадку. Возвращает: (Parsed, неразборчивые identity «тикер: identity» всех
/// своих цепей). Ошибки: BadResponse — как contracts::parse и parse_with.
pub fn parse_all(
    info: &Value,
    data: &Value,
    n: &Names,
    venue: &VenueId,
) -> GatewayResult<(Parsed, Vec<String>)> {
    let mut maps: Vec<HashMap<String, Address>> = vec![];
    let mut bad = vec![];
    for (_, chain) in &n.pairs {
        let c = super::contracts::parse(info, chain)?;
        maps.push(c.by_ticker);
        bad.extend(c.bad);
    }
    let own: Vec<(&str, &HashMap<String, Address>)> = n
        .pairs
        .iter()
        .zip(&maps)
        .map(|((net, _), m)| (net.as_str(), m))
        .collect();
    Ok((parse_with(data, &own, venue, n.min_ok)?, bad))
}
