//! Только чтение ERC20 balanceOf; результат хранит отдельно address и storage key.
use super::*;
use serde::Deserialize;
use std::collections::BTreeSet;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Access {
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
    pub(super) fn accounts(&self) -> impl Iterator<Item = Address> + '_ {
        self.access_list.iter().map(|r| r.address)
    }
    pub(super) fn completed(&self) -> bool {
        self.error.is_none()
    }
    fn keys(&self) -> BTreeSet<(Address, B256)> {
        self.access_list
            .iter()
            .flat_map(|r| r.storage_keys.iter().map(move |k| (r.address, *k)))
            .collect()
    }
    fn others(&self, t: Address) -> BTreeSet<Address> {
        self.access_list
            .iter()
            .map(|r| r.address)
            .filter(|a| *a != t)
            .collect()
    }
}
#[derive(Clone, Copy)]
pub(super) struct Reads {
    pub(super) other: Option<Address>,
    pub(super) implementation: bool,
    pub(super) admin: bool,
}
impl Reads {
    pub(super) fn matches_implementation(self, word: B256) -> bool {
        self.other.is_some_and(|a| a.into_word() == word)
    }
}
pub(super) async fn call(ctx: &Ctx, call: &simulate::Call, h: Head) -> Result<Access, String> {
    rpc::call(
        ctx,
        "eth_createAccessList",
        json!([{"from":call.from,"to":call.to,"data":call.input},h.selector()]),
    )
    .await
    .map_err(|e| e.message)
}
#[derive(Clone, Copy)]
pub(super) struct Location {
    pub(super) storage: Address,
    pub(super) key: B256,
}
impl Location {
    pub(super) fn bounded(self, max: U256) -> BalanceField {
        BalanceField {
            storage: self.storage,
            key: self.key,
            max,
        }
    }
}
pub(super) fn compare(
    a: &Access,
    b: &Access,
    t: Address,
) -> Result<([Location; 2], Reads), String> {
    if let Some(e) = a.error.as_ref().or(b.error.as_ref()) {
        return Err(format!("balanceOf в списке доступа: {e}"));
    }
    let (a_keys, b_keys) = (a.keys(), b.keys());
    let mut implementation = false;
    let mut admin = false;
    for &(addr, key) in a_keys.intersection(&b_keys) {
        if addr == t && key == IMPL_SLOT {
            implementation = true
        } else if addr == t && key == ADMIN_SLOT {
            admin = true
        } else {
            return Err("общие чтения balanceOf выходят за подтверждённые поля прокси".into());
        }
    }
    let one = |left: &BTreeSet<_>, right: &BTreeSet<_>| {
        let mut keys = left.difference(right);
        let Some(&(storage, key)) = keys.next() else {
            return Err("нет отдельного ключа держателя".to_string());
        };
        if keys.next().is_some() {
            return Err("несколько отдельных ключей держателя".into());
        }
        if [IMPL_SLOT, ADMIN_SLOT, BEACON_SLOT].contains(&key) {
            return Err("служебный слот прокси".into());
        }
        Ok(Location { storage, key })
    };
    let fields = [one(&a_keys, &b_keys)?, one(&b_keys, &a_keys)?];
    if fields[0].storage != fields[1].storage {
        return Err("разные контракты хранения для двух держателей".into());
    }
    let others = a.others(t);
    if others.len() > 1 || others != b.others(t) {
        return Err("неоднозначный код чтения balanceOf".into());
    }
    let other = others.first().copied();
    if (implementation && other.is_none())
        || (fields[0].storage != t && Some(fields[0].storage) != other)
    {
        return Err("адрес хранения не принадлежит проверяемому вызову".into());
    }
    Ok((
        fields,
        Reads {
            other,
            implementation,
            admin,
        },
    ))
}
