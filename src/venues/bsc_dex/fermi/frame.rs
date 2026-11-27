//! Публичное состояние оракула Fermi для одного строителя и одного слота Ethereum.
//! Форматы Titan https://docs.titanbuilder.xyz/propamms и BuilderNet https://buildernet.org/docs/api .
use super::Deployment;
use alloy_primitives::{Address, B256, U64};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const GENESIS: u64 = 1_606_824_023;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn name(self) -> &'static str {
        match self {
            Self::Titan => "titan",
            Self::BuilderNet => "buildernet",
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

#[derive(Clone)]
pub(super) struct Frame {
    pub deployment: Deployment,
    pub target: u64,
    pub slot: u64,
    pub timestamp: u64,
    pub diff: BTreeMap<B256, B256>,
}

impl Frame {
    /// Принимает только фиксированный адрес движка и 32-байтовые ключи и значения stateDiff.
    /// Прочие счета, баланс, nonce, code и полное замещение storage из кадра отбрасываются.
    pub fn read(builder: Builder, result: &Value, deployment: Deployment) -> Option<Self> {
        let (row, target, slot) = match builder {
            Builder::Titan => {
                let mut matching = result.get("venues")?.as_array()?.iter().filter(|r| {
                    r["maker"] == "Fermi"
                        && r["quoteAddress"]
                            .as_str()
                            .and_then(|v| v.parse::<Address>().ok())
                            == Some(deployment.router)
                });
                let r = matching.next()?;
                if matching.next().is_some() {
                    return None;
                }
                (r, r["blockNumber"].as_u64()?, r["slot"].as_u64()?)
            }
            Builder::BuilderNet => (
                result.get(deployment.router.to_string().to_lowercase())?,
                serde_json::from_value::<U64>(result.get("blockNumber")?.clone())
                    .ok()?
                    .to::<u64>(),
                result.get("slot")?.as_u64()?,
            ),
        };
        Self::from_row(row, target, slot, deployment)
    }

    pub(super) fn from_row(
        row: &Value,
        target: u64,
        slot: u64,
        deployment: Deployment,
    ) -> Option<Self> {
        let overrides = row.get("stateOverride")?.as_object()?;
        let engine = overrides.get(&deployment.engine.to_string().to_lowercase())?;
        let diff = serde_json::from_value::<Diff>(engine.clone())
            .ok()?
            .state_diff;
        if diff.is_empty() || diff.len() > 128 {
            return None;
        }
        let timestamp = GENESIS.checked_add(slot.checked_mul(12)?)?;
        Some(Self {
            deployment,
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
        json!({(format!("{:#x}", self.deployment.engine)):{"stateDiff":self.diff}})
    }
}
