//! Идентичность дополнительных площадок Base и Ethereum. Настройки оператор вводит в UI.
//! Сверяется при загрузке и записи настроек. Схема API отдаёт те же данные редактору.
//! Источники: https://docs.base.org/get-started/connect-to-base и https://eips.ethereum.org/EIPS/eip-155 .

use serde::Serialize;
use serde_json::Value;

use super::{scope::Scope, store::Store};

/// Канонические идентификаторы. Наличие записи в справочнике не регистрирует площадку.
#[derive(Serialize)]
pub struct ChainMeta {
    pub venue: &'static str,
    pub chain: &'static str,
    pub evm_chain_id: u64,
    pub gas_mode: &'static str,
    pub public_channel: &'static str,
}

pub const ETHEREUM_MAINNET: u64 = 1;

pub const PROFILES: &[ChainMeta] = &[
    ChainMeta {
        venue: "base_dex",
        chain: "base",
        evm_chain_id: 8453,
        gas_mode: "op_stack_eip1559",
        public_channel: "base_public",
    },
    ChainMeta {
        venue: "eth_dex",
        chain: "ethereum",
        evm_chain_id: ETHEREUM_MAINNET,
        gas_mode: "ethereum_eip1559",
        public_channel: "ethereum_public",
    },
];

/// Ожидаемая идентичность для известной площадки.
pub fn profile(venue: &str) -> Option<&'static ChainMeta> {
    PROFILES.iter().find(|p| p.venue == venue)
}

/// Проверенная идентичность физической сети сохраняется при другом имени площадки.
pub fn by_chain_id(id: u64) -> Option<&'static ChainMeta> {
    PROFILES.iter().find(|p| p.evm_chain_id == id)
}

/// Модели подтверждённых сетей прежнего адаптера. Источники записанных RPC — tests/data/base_ethereum_networks.json,
/// tests/arbitrum_sub_dex_live.rs и tests/data/native_2026-10-25. Области настроек этих площадок исторически свободны.
pub fn legacy_network_mode(id: u64) -> Option<&'static str> {
    match id {
        56 => Some("legacy"),
        42161 => Some("eip1559_l1"),
        _ => None,
    }
}
/// Строгие модели требуют отдельного адреса узла, независимо от имени площадки.
pub fn isolated_transport(s: &Store, venue: &str) -> bool {
    if profile(venue).is_some() {
        return true;
    }
    let scope = Scope::Venue(venue.into());
    s.get_exact("dex.evm_chain_id", &scope)
        .and_then(|v| v.as_u64())
        .and_then(by_chain_id)
        .is_some()
        || s.get_exact("dex.gas_mode", &scope)
            .is_some_and(|v| matches!(v.as_str(), Some("op_stack_eip1559" | "ethereum_eip1559")))
}

/// Идентичность площадки по её сохранённой физической сети либо по защищённому имени профиля.
pub fn configured(s: &Store, venue: &str) -> Option<&'static ChainMeta> {
    profile(venue).or_else(|| {
        s.get_exact("dex.evm_chain_id", &Scope::Venue(venue.into()))
            .and_then(|n| n.as_u64())
            .and_then(by_chain_id)
    })
}

/// Настройки конкретной сети с исторически допустимой общей областью BSC.
pub fn local_only(name: &str) -> bool {
    matches!(
        name,
        "bsc.node_http_url"
            | "bsc.node_http_url_backup"
            | "bsc.node_ws_url"
            | "bsc.node_ws_url_backup"
            | "bsc.receipts_node_urls"
            | "index.network"
            | "venue.enabled"
    )
}

impl ChainMeta {
    /// Сверяет значение, включая уже сохранённое в прежней версии. Агрегатор подписывает только BSC.
    pub fn check(&self, name: &str, value: Option<&Value>) -> Result<(), String> {
        match name {
            "dex.chain" if value.and_then(Value::as_str) != Some(self.chain) => Err(format!(
                "{}: требуется dex.chain = {}",
                self.venue, self.chain
            )),
            "dex.evm_chain_id" if value.and_then(Value::as_u64) != Some(self.evm_chain_id) => {
                Err(format!(
                    "{}: требуется dex.evm_chain_id = {}",
                    self.venue, self.evm_chain_id
                ))
            }
            "dex.gas_mode" if value.and_then(Value::as_str) != Some(self.gas_mode) => Err(format!(
                "{}: требуется dex.gas_mode = {}",
                self.venue, self.gas_mode
            )),
            "exec.via_kyber" if value.and_then(Value::as_bool) == Some(true) => Err(format!(
                "{}: адаптер KyberSwap подписывает транзакции сети BSC (56)",
                self.venue
            )),
            _ => Ok(()),
        }
    }

    /// Конфликтующая идентичность другой площадки. Нельзя регистрировать два владельца сети.
    pub fn overlaps(&self, name: &str, value: Option<&Value>) -> bool {
        match name {
            "dex.chain" => value.and_then(Value::as_str) == Some(self.chain),
            "dex.evm_chain_id" => value.and_then(Value::as_u64) == Some(self.evm_chain_id),
            _ => false,
        }
    }

    /// Перед регистрацией требует явные идентификаторы и историческую BSC для записей без сети.
    pub fn ready(&self, s: &Store) -> Result<(), String> {
        self.ready_as(s, self.venue)
    }

    pub fn ready_as(&self, s: &Store, venue: &str) -> Result<(), String> {
        let scope = Scope::Venue(venue.into());
        for name in [
            "dex.chain",
            "dex.evm_chain_id",
            "dex.gas_mode",
            "exec.via_kyber",
        ] {
            self.check(name, s.get_exact(name, &scope).as_ref())?;
        }
        if s.get_exact("dex.default_chain", &Scope::Global)
            .as_ref()
            .and_then(Value::as_str)
            != Some("bsc")
            || s.get_exact("dex.chain", &Scope::Venue("bsc_dex".into()))
                .as_ref()
                .and_then(Value::as_str)
                != Some("bsc")
        {
            return Err(format!(
                "{}: для исторических записей задайте dex.default_chain = bsc и dex.chain = bsc у bsc_dex",
                self.venue
            ));
        }
        Ok(())
    }
}

/// Валюта существующего портфельного учёта.
pub const LEDGER_QUOTE: &str = "USDT";

pub fn uses_settlement_contract(s: &Store, venue: &str) -> bool {
    s.get_exact("dex.settlement_contract", &Scope::Venue(venue.into()))
        .is_some()
}

/// Только явно сохранённый оператором адрес. При новой установке адрес отсутствует.
pub fn settlement_contract(
    s: &Store,
    venue: &str,
    _chain: &str,
    _ticker: &str,
) -> Result<Option<alloy_primitives::Address>, String> {
    if !uses_settlement_contract(s, venue) {
        return Ok(None);
    }
    let at = super::read::At {
        venue: Some(venue),
        coin: None,
    };
    let addr = s
        .address("dex.settlement_contract", at)
        .map_err(|e| e.message)?;
    if addr.is_zero() {
        return Err("Нулевой адрес dex.settlement_contract".into());
    }
    Ok(Some(addr))
}
