//! Файл: venues/bsc_dex/pools/families.rs
//! Что делает: виды пулов, которые умеет адаптер (это перечень его модулей, ARCHITECTURE.md): id области
//! настроек «DEX», имя DEX, математика (PoolKind), протокол (разбор событий и состояния) и официальные ссылки
//! на адреса контрактов. Адресов здесь нет — их вводит Тигран в настройках dex.* по виду пула.
//! Кто вызывает: mod.rs (dex_families), pools/* (выбор разбора по протоколу).
//! От чего зависит: core/types (PoolKind), gateway/contract (DexFamily).
//! Настройки: dex.enabled [DEX], dex.pool_enabled [вид пула] — читает pools/market.rs.
//!
//! Виды — решения 13 и 31; ids — ARCHITECTURE.md, «Раскладка адаптера DEX».

use crate::core::types::PoolKind;
use crate::gateway::contract::DexFamily;

/// Протокол вида пула: от него зависит ABI событий и чтение состояния.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proto {
    /// V2 (PancakeSwap и Uniswap — одинаковые события Sync и getReserves).
    V2,
    /// PancakeSwap V3: slot0.feeProtocol uint32, Swap из 9 полей.
    PancakeV3,
    /// Uniswap V3: slot0.feeProtocol uint8, Swap из 7 полей.
    UniswapV3,
    /// PancakeSwap Infinity CL: геттеры у CLPoolManager.
    InfinityCl,
    /// PancakeSwap Infinity Bin: геттеры у BinPoolManager.
    InfinityBin,
    /// Uniswap V4: состояние через StateView.
    UniswapV4,
}

/// Вид пула адаптера.
#[derive(Debug)]
pub struct Family {
    /// id области «DEX» (настройки dex.pool_enabled и адреса контрактов).
    pub id: &'static str,
    /// Имя DEX — id области «DEX» для dex.enabled.
    pub dex: &'static str,
    pub kind: PoolKind,
    pub proto: Proto,
    pub title: &'static str,
    /// Официальные страницы адресов контрактов.
    pub docs: &'static [&'static str],
}

const PCS_V2: &[&str] = &["https://developer.pancakeswap.finance/contracts/v2/addresses"];
const PCS_V3: &[&str] = &["https://developer.pancakeswap.finance/contracts/v3/addresses"];
const PCS_INF: &[&str] =
    &["https://developer.pancakeswap.finance/contracts/infinity/resources/addresses"];
const UNI_V2: &[&str] = &["https://developers.uniswap.org/docs/protocols/v2/deployments"];
const UNI_V3: &[&str] =
    &["https://developers.uniswap.org/docs/protocols/v3/deployments/v3-bnb-deployments"];
const UNI_V4: &[&str] = &["https://developers.uniswap.org/docs/protocols/v4/deployments"];

/// Все виды пулов адаптера.
#[rustfmt::skip]
pub static FAMILIES: &[Family] = &[
    Family { id: "pancakeswap_v2", dex: "pancakeswap", kind: PoolKind::V2, proto: Proto::V2, title: "PancakeSwap V2", docs: PCS_V2 },
    Family { id: "pancakeswap_v3", dex: "pancakeswap", kind: PoolKind::V3, proto: Proto::PancakeV3, title: "PancakeSwap V3", docs: PCS_V3 },
    Family { id: "pancakeswap_infinity_cl", dex: "pancakeswap", kind: PoolKind::Cl, proto: Proto::InfinityCl, title: "PancakeSwap Infinity CL", docs: PCS_INF },
    Family { id: "pancakeswap_infinity_bin", dex: "pancakeswap", kind: PoolKind::Bin, proto: Proto::InfinityBin, title: "PancakeSwap Infinity Bin", docs: PCS_INF },
    Family { id: "uniswap_v2", dex: "uniswap", kind: PoolKind::V2, proto: Proto::V2, title: "Uniswap V2", docs: UNI_V2 },
    Family { id: "uniswap_v3", dex: "uniswap", kind: PoolKind::V3, proto: Proto::UniswapV3, title: "Uniswap V3", docs: UNI_V3 },
    Family { id: "uniswap_v4", dex: "uniswap", kind: PoolKind::Cl, proto: Proto::UniswapV4, title: "Uniswap V4", docs: UNI_V4 },
];

/// Вид пула по id. None — такого вида нет.
pub fn find(id: &str) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| f.id == id)
}

/// Список для контракта (dex_families): что показать в интерфейсе. Ошибок нет.
pub fn for_ui() -> Vec<DexFamily> {
    FAMILIES
        .iter()
        .map(|f| DexFamily {
            dex: f.dex.to_string(),
            family: f.id.to_string(),
            kind: f.kind,
            title: f.title.to_string(),
            docs: f.docs.iter().map(|s| s.to_string()).collect(),
        })
        .collect()
}
