//! Файл: gateway/own_nets.rs
//! Что делает: решение 431 (шаг 3 двух сетей) — имена «своих» сетей биржи: как биржа называет сеть каждой сети ядра.
//! Правило имени настройки одно на всё: сеть по умолчанию (монета без сети, core/chain.rs) — venue.bsc_network (как
//! раньше); другая сеть `c` (id сети — dex.chain её площадки DEX) — `venue.<c>_network`; для Arbitrum (dex.chain =
//! «arbitrum») — venue.arbitrum_network. Адаптер биржи ставит адрес контракта у записи КАЖДОЙ своей сети (OwnNets), а к
//! какой сети ядра относится запись, решает только ядро (core/net_chain.rs). Сеть запроса (Asset.chain) → имя сети биржи
//! — name_of (адрес депозита, история депозитов).
//! Кто вызывает: адаптеры бирж (wallet.rs, networks_parse.rs, fund_move.rs, deposits.rs), core/net_chain.rs,
//! core/transfers.rs, core/rebuild*.rs.
//! От чего зависит: settings (store, read), core/types (ChainId), gateway/errors.
//! Настройки: venue.bsc_network (обязательная, как раньше), venue.arbitrum_network (необязательная) — по площадке.

use crate::core::types::ChainId;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::errors::GatewayResult;

/// Имя сети BSC у биржи (сеть по умолчанию; решение 108).
pub const BSC: &str = "venue.bsc_network";

/// Имена сетей других сетей ядра у биржи — по правилу `venue.<id сети>_network` (setting_of). Каждое имя — запись
/// каталога настроек (тест в этом файле). Новая сеть = новая строка здесь и в каталоге.
pub const OTHER: [&str; 1] = ["venue.arbitrum_network"];

/// Что делает: имя настройки «имя сети у биржи» для сети ядра. Принимает: сеть в каноническом виде (None — сеть по
/// умолчанию). Возвращает: venue.bsc_network или venue.<id>_network. Ошибок нет.
pub fn setting_of(chain: Option<&ChainId>) -> String {
    match chain {
        None => BSC.to_string(),
        Some(c) => format!("venue.{}_network", c.0),
    }
}

/// Что делает: меняет ли настройка имена своих сетей (толчок справочника сетей и пересборки). Ошибок нет.
pub fn affects(name: &str) -> bool {
    name == BSC || OTHER.contains(&name)
}

/// Имена своих сетей биржи: первая — сеть BSC (venue.bsc_network), дальше — заданные имена других сетей.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnNets {
    names: Vec<String>,
}

impl OwnNets {
    /// Что делает: своя ли сеть (BSC или другая заданная) — у неё адрес контракта ставится. Ошибок нет.
    pub fn has(&self, network: &str) -> bool {
        self.names.iter().any(|n| n == network)
    }

    /// Что делает: это сеть BSC (venue.bsc_network) — для журнальных пометок «сети BSC …» как раньше. Ошибок нет.
    pub fn is_bsc(&self, network: &str) -> bool {
        self.names.first().is_some_and(|n| n == network)
    }

    /// Что делает: имя сети BSC. Ошибок нет.
    pub fn bsc(&self) -> &str {
        self.names.first().map_or("", String::as_str)
    }

    /// Что делает: все имена (BSC первой). Ошибок нет.
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

/// Одно имя — только сеть BSC (как до решения 431; разборы в тестах зовутся с «BEP20»).
impl From<&str> for OwnNets {
    fn from(bsc: &str) -> Self {
        OwnNets {
            names: vec![bsc.to_string()],
        }
    }
}

impl From<&String> for OwnNets {
    fn from(bsc: &String) -> Self {
        OwnNets::from(bsc.as_str())
    }
}

impl From<String> for OwnNets {
    fn from(bsc: String) -> Self {
        OwnNets { names: vec![bsc] }
    }
}

impl From<&OwnNets> for OwnNets {
    fn from(n: &OwnNets) -> Self {
        n.clone()
    }
}

/// Что делает: имена своих сетей биржи из настроек. Принимает: хранилище, область площадки.
/// Возвращает: OwnNets — BSC (обязательна, как раньше; в режиме подсказки решения 200 — пустая) и заданные непустые
/// имена других сетей (не заданы — не входят). Ошибки: NotConfigured — не задана venue.bsc_network.
pub fn read(s: &Store, at: At) -> GatewayResult<OwnNets> {
    let mut names = vec![s.text(BSC, at)?];
    for o in OTHER {
        if let Ok(n) = s.text(o, at)
            && !n.is_empty()
            && !names.contains(&n)
        {
            names.push(n);
        }
    }
    Ok(OwnNets { names })
}

/// Что делает: имя сети биржи для сети монеты запроса. Принимает: хранилище, область площадки, сеть монеты
/// (Asset.chain; None — сеть по умолчанию). Возвращает: имя. Ошибки: NotConfigured — настройка не задана.
pub fn name_of(s: &Store, at: At, chain: Option<&ChainId>) -> GatewayResult<String> {
    s.text(&setting_of(chain), at)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Имена других сетей — по правилу setting_of и есть в каталоге настроек.
    #[test]
    fn other_follow_rule_and_catalog() {
        assert_eq!(setting_of(None), BSC);
        assert_eq!(
            setting_of(Some(&ChainId("arbitrum".into()))),
            "venue.arbitrum_network"
        );
        for o in OTHER {
            assert!(crate::settings::catalog::find(o).is_some(), "{o}");
            assert!(affects(o));
        }
    }

    /// Одно имя — только BSC.
    #[test]
    fn from_str_is_bsc_only() {
        let n = OwnNets::from("BEP20");
        assert!(n.has("BEP20") && n.is_bsc("BEP20") && !n.has("ARBITRUM"));
        assert_eq!(n.bsc(), "BEP20");
    }
}
