//! Файл: venues/gate/deposit_plan.rs
//! Что делает: решение 387 — чистая логика фонового круга сведений депозита Gate (deposit_info.rs), без сети:
//! - plan: какие монеты подписки спрашивать и в каком порядке. Монета, у которой по последнему справочнику сетей Gate
//!   (GET /spot/currencies, флаг deposit_disabled сети BSC) ввод в сети BSC закрыт, не спрашивается: Gate на адрес
//!   депозита такой монеты отвечает отказом (живой факт 17.11: «HTTP 400, INVALID_PARAM_VALUE: deposit disabled» у 728
//!   из 1070 монет; на записи справочника 04.11 ввод в BSC закрыт у 724 из 1075 монет с сетью BSC — tests/gate_real_data.rs),
//!   а ядро на такую биржу и так не переводит (Presence::can_receive). Откроется ввод — монета попадёт в следующий
//!   круг сама (справочник перечитывается). Монеты без сети BSC в справочнике тоже не спрашиваются. Справочника ещё нет
//!   — спрашиваются все, как раньше. Порядок: сначала монеты с неизвестным минимумом, затем — уже известные
//!   (перепроверка). Монеты, нужные расчёту прямо сейчас, ждать круга не должны — их запрашивает ядро по требованию
//!   (core/deposit_min_ask.rs → coin_networks_of → deposit_min.rs ensure); адаптер не знает направлений ядра.
//! - transient: сбой «временной» причины (секреты закрыты, связь, срок, лимит) — монета повторяется, как только причина
//!   устранена; иные сбои (ввод закрыт, адреса нет, memo) — до следующего полного круга.
//! - stops_round: секреты закрыты — остальные запросы круга упадут так же; круг прекращается, монеты — в повтор.
//! Кто вызывает: deposit_info.rs, deposit_min.rs, tests/gate_deposit_plan.rs.
//! От чего зависит: core/types, gateway/errors.
//! Настройки: нет (сеть BSC передаёт вызывающий — venue.bsc_network).

use std::collections::{HashMap, HashSet};

use crate::core::types::{Asset, CoinNetworkInfo};
use crate::gateway::errors::ErrorCode;

/// Что спросить в круге.
#[derive(Debug, Default)]
pub struct Plan {
    /// Монеты к запросу — в порядке запросов.
    pub ask: Vec<Asset>,
    /// Не спрашиваются: ввод в сети BSC закрыт по справочнику сетей.
    pub closed: usize,
    /// Не спрашиваются: сети BSC у монеты в справочнике нет.
    pub no_bsc: usize,
}

/// Что делает: план круга (см. шапку). Принимает: монеты подписки, последний справочник сетей Gate (None — ещё нет),
/// название сети BSC, «минимум монеты уже известен», только эти тикеры (повтор после сбоя; None — все).
/// Возвращает: план. Ошибок нет.
pub fn plan(
    assets: &[Asset],
    dir: Option<&[CoinNetworkInfo]>,
    bsc: &str,
    known: &dyn Fn(&str) -> bool,
    only: Option<&HashSet<String>>,
) -> Plan {
    // Тикер → ввод в сети BSC открыт.
    let open: Option<HashMap<&str, bool>> = dir.map(|l| {
        l.iter()
            .filter(|n| n.network == bsc)
            .map(|n| (n.asset.ticker.as_str(), n.deposit_enabled))
            .collect()
    });
    let mut p = Plan::default();
    let mut later = Vec::new();
    for a in assets {
        if only.is_some_and(|o| !o.contains(&a.ticker)) {
            continue;
        }
        match open.as_ref().map(|m| m.get(a.ticker.as_str()).copied()) {
            Some(None) => {
                p.no_bsc += 1;
                continue;
            }
            Some(Some(false)) => {
                p.closed += 1;
                continue;
            }
            _ => {}
        }
        if known(&a.ticker) {
            later.push(a.clone());
        } else {
            p.ask.push(a.clone());
        }
    }
    p.ask.extend(later);
    p
}

/// Сбой временной причины — повторить, как только она устранена. Принимает: код ошибки шлюза. Ошибок нет.
pub fn transient(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::SecretsLocked
            | ErrorCode::Unreachable
            | ErrorCode::Timeout
            | ErrorCode::RateLimited
            | ErrorCode::VenueNotConnected
    )
}

/// Сбой, после которого остальные запросы круга упадут так же (секреты закрыты). Принимает: код. Ошибок нет.
pub fn stops_round(code: ErrorCode) -> bool {
    code == ErrorCode::SecretsLocked
}
