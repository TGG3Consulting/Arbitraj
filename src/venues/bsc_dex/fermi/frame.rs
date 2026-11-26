//! Публичное состояние оракула Fermi для одного строителя и одного слота Ethereum.
//! Форматы Titan https://docs.titanbuilder.xyz/propamms и BuilderNet https://buildernet.org/docs/api .
use alloy_primitives::{Address, B256, U64, address};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) const ROUTER: Address = address!("f6d5b2fefec7b420d9990b95790adfd3d4c4f977");
pub(super) const ENGINE: Address = address!("10e1a2b624e14ac59426199e520320313a98b0be");
const GENESIS: u64 = 1_606_824_023;

#[derive(Clone, Copy)]
pub(super) enum Builder {
    Titan,
    BuilderNet,
}
impl Builder {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "titan" => Some(Self::Titan),
            "buildernet" => Some(Self::BuilderNet),
            _ => None,
        }
    }
    pub fn data(self) -> (&'static str, &'static str) {
        match self {
            Self::Titan => (
                "https://eu.data.titanbuilder.xyz/data",
                "titan_getPammVenues",
            ),
            Self::BuilderNet => (
                "https://rpc.buildernet.org/pamm/rpc",
                "buildernet_getPammStateOverrides",
            ),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Diff {
    state_diff: BTreeMap<B256, B256>,
}

pub(super) struct Frame {
    pub target: u64,
    pub slot: u64,
    pub timestamp: u64,
    pub diff: BTreeMap<B256, B256>,
}

impl Frame {
    /// Принимает только фиксированный адрес движка и 32-байтовые ключи и значения stateDiff.
    /// Прочие счета, баланс, nonce, code и полное замещение storage из кадра отбрасываются.
    pub fn read(builder: Builder, result: &Value) -> Option<Self> {
        let (row, target, slot) = match builder {
            Builder::Titan => {
                let mut matching = result.get("venues")?.as_array()?.iter().filter(|r| {
                    r["maker"] == "Fermi"
                        && r["quoteAddress"]
                            .as_str()
                            .and_then(|v| v.parse::<Address>().ok())
                            == Some(ROUTER)
                });
                let r = matching.next()?;
                if matching.next().is_some() {
                    return None;
                }
                (r, r["blockNumber"].as_u64()?, r["slot"].as_u64()?)
            }
            Builder::BuilderNet => (
                result.get(ROUTER.to_string().to_lowercase())?,
                serde_json::from_value::<U64>(result.get("blockNumber")?.clone())
                    .ok()?
                    .to::<u64>(),
                result.get("slot")?.as_u64()?,
            ),
        };
        let overrides = row.get("stateOverride")?.as_object()?;
        let engine = overrides.get(&ENGINE.to_string().to_lowercase())?;
        let diff = serde_json::from_value::<Diff>(engine.clone())
            .ok()?
            .state_diff;
        if diff.is_empty() || diff.len() > 128 {
            return None;
        }
        let timestamp = GENESIS.checked_add(slot.checked_mul(12)?)?;
        Some(Self {
            target,
            slot,
            timestamp,
            diff,
        })
    }

    /// Разрешает только следующий номер блока и открытое окно Ethereum-слота.
    /// При пропущенных слотах родитель может быть старше 12 с.
    pub fn current(&self, parent: u64, parent_ts: u64, now: u64) -> bool {
        parent.checked_add(1) == Some(self.target)
            && parent_ts < self.timestamp
            && now >= parent_ts
            && now < self.timestamp
            && self.timestamp - now <= 12
    }

    pub fn overrides(&self) -> Value {
        json!({(format!("{ENGINE:#x}")):{"stateDiff":self.diff}})
    }
}
