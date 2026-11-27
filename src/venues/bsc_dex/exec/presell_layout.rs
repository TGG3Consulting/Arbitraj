//! Полнословное поле balances подтверждается разметкой компилятора и фактическим
//! кодом, включая реализацию прокси. Лишь затем список чтений balanceOf служит
//! независимой проверкой конкретного ключа. Источники — verified_layouts.json.
use alloy_primitives::{Address, B256, Bytes, U256, b256, keccak256};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::LazyLock};

use super::super::{
    check::{probe_calls, simulate},
    ctx::Ctx,
    node::rpc,
    quote::white_state::Head,
};
use super::presell_slot::{self, Layout, Slot};

const IMPL_SLOT: B256 = b256!("360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc");
#[derive(Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Declaration {
    Balances { slot: u64 },
    TransparentErc1967,
}
#[derive(Deserialize)]
struct Deployment {
    chain_id: u64,
    address: Address,
    runtime_hash: B256,
    layout: Declaration,
    market_token: bool,
}
static DEPLOYMENTS: LazyLock<Vec<Deployment>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../../config/presell_erc20_layouts.json"
    ))
    .expect("verified ERC20 compiler layouts")
});
pub(super) fn seed(t: Address) -> bool {
    DEPLOYMENTS
        .iter()
        .any(|d| d.chain_id == 56 && d.market_token && d.address == t)
}
fn declaration(hash: B256) -> Option<Declaration> {
    DEPLOYMENTS
        .iter()
        .find(|d| d.chain_id == 56 && d.runtime_hash == hash)
        .map(|d| d.layout)
}
fn full_word(code: B256) -> Option<Slot> {
    let Declaration::Balances { slot } = declaration(code)? else {
        return None;
    };
    Some(Slot {
        index: slot,
        layout: Layout::Solidity,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Access {
    access_list: Vec<Item>,
    error: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    address: Address,
    storage_keys: Vec<B256>,
}
impl Access {
    fn keys(&self) -> BTreeSet<(Address, B256)> {
        self.access_list
            .iter()
            .flat_map(|r| r.storage_keys.iter().map(move |k| (r.address, *k)))
            .collect()
    }
}

fn matches(
    a: &Access,
    b: &Access,
    token: Address,
    keys: [B256; 2],
    implementation: Option<Address>,
) -> bool {
    if a.error.is_some() || b.error.is_some() || keys[0] == keys[1] {
        return false;
    }
    if let Some(imp) = implementation
        && [a, b]
            .iter()
            .any(|l| !l.access_list.iter().any(|r| r.address == imp))
    {
        return false;
    }
    let (left, right) = (a.keys(), b.keys());
    left.difference(&right).copied().eq([(token, keys[0])])
        && right.difference(&left).copied().eq([(token, keys[1])])
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Identity {
    runtime: B256,
    implementation: Option<(Address, B256)>,
}
pub(super) struct Proof {
    pub(super) identity: Identity,
    pub(super) slot: Slot,
    pub(super) balance: U256,
}
async fn code(ctx: &Ctx, address: Address, at: &Value) -> Result<B256, String> {
    let bytes: Bytes = rpc::call(ctx, "eth_getCode", json!([address, at]))
        .await
        .map_err(|e| e.message)?;
    Ok(keccak256(bytes))
}

/// Один такой снимок кода и привязки на контракт и канонический блок для всех кошельков.
pub(super) async fn deployment(ctx: &Ctx, t: Address, h: Head) -> Result<Option<Identity>, String> {
    let at = h.selector();
    let runtime = code(ctx, t, &at).await?;
    let implementation = if declaration(runtime) == Some(Declaration::TransparentErc1967) {
        let word: B256 = rpc::call(ctx, "eth_getStorageAt", json!([t, IMPL_SLOT, at]))
            .await
            .map_err(|e| e.message)?;
        let imp = Address::from_word(word);
        if imp.is_zero() || imp.into_word() != word {
            return Err("некорректная реализация прокси".into());
        }
        Some((imp, code(ctx, imp, &at).await?))
    } else {
        None
    };
    let id = Identity {
        runtime,
        implementation,
    };
    Ok(full_word(implementation.map_or(runtime, |(_, hash)| hash)).map(|_| id))
}

fn balance_word(balance: &[u8], raw: B256) -> Result<U256, String> {
    if balance != raw.as_slice() {
        return Err("слово баланса отличается от balanceOf".into());
    }
    Ok(U256::from_be_bytes(raw.0))
}

pub(super) async fn verify(
    ctx: &Ctx,
    t: Address,
    wallet: Address,
    h: Head,
    identity: Identity,
    previous: Option<Identity>,
) -> Result<Proof, String> {
    let at = h.selector();
    let implementation = identity.implementation;
    let slot = full_word(implementation.map_or(identity.runtime, |(_, hash)| hash))
        .ok_or("неизвестная разметка")?;
    let control = if wallet == Address::ZERO {
        return Err("нет адреса кошелька".into());
    } else {
        Address::ZERO
    };
    let call = probe_calls::balance(wallet, t, wallet);
    let tx = json!({"from":wallet,"to":t,"data":call.input});
    let other = json!({"from":wallet,"to":t,"data":probe_calls::balance(wallet,t,control).input});
    let keys = [
        presell_slot::key(slot, wallet),
        presell_slot::key(slot, control),
    ];
    let (balance, raw) = tokio::try_join!(
        rpc::call::<Bytes>(ctx, "eth_call", json!([tx, at])),
        rpc::call::<B256>(ctx, "eth_getStorageAt", json!([t, keys[0], at])),
    )
    .map_err(|e| e.message)?;
    let have = balance_word(&balance, raw)?;
    if previous == Some(identity) {
        return Ok(Proof {
            identity,
            slot,
            balance: have,
        });
    }
    let (own, second) = tokio::try_join!(
        rpc::call::<Access>(ctx, "eth_createAccessList", json!([tx, at])),
        rpc::call::<Access>(ctx, "eth_createAccessList", json!([other, at])),
    )
    .map_err(|e| e.message)?;
    if !matches(
        &own,
        &second,
        t,
        keys,
        implementation.map(|(address, _)| address),
    ) {
        return Err("balanceOf двух держателей не подтвердил одно поле и реализацию".into());
    }
    let marker = have ^ U256::from(1);
    let over = presell_slot::credit(t, wallet, slot, marker);
    let base = simulate::Base::for_chain(Some(56), h.number.to(), h.timestamp.to());
    let res = simulate::run_anchored(ctx, &[call], &[], &[over], base, Some(h.hash))
        .await
        .map_err(|e| e.message)?;
    if res.first().filter(|r| r.ok()).and_then(probe_calls::u256) != Some(marker) {
        return Err("одиночная подмена подтверждённого поля не дала заданный баланс".into());
    }
    Ok(Proof {
        identity,
        slot,
        balance: have,
    })
}

#[cfg(test)]
impl Identity {
    pub(super) fn test_direct(runtime: B256) -> Self {
        Self {
            runtime,
            implementation: None,
        }
    }
}

#[cfg(test)]
#[path = "presell_layout_tests.rs"]
mod tests;
