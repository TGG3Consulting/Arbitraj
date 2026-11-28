//! Старый журнал PMM не содержал снимка ролей. Допустим только тот полный исходный Ethereum-домен,
//! для которого работал прежний writer. Пользовательская конфигурация и название площадки его не заменяют.
use super::super::{roles::FrozenRoles, wire::Terms};
use alloy_primitives::{B256, b256, keccak256};
use serde::{Deserialize, Deserializer, de::Error};
use serde_json::Value;
use std::sync::OnceLock;

const ORIGINAL: B256 = b256!("e1163c1b43fe6af74db972f9189f34c899ba798df7b9bfd980a6754775e6d58f");

pub(super) fn fingerprint(r: FrozenRoles) -> B256 {
    let mut bytes = Vec::with_capacity(8 + 7 * (20 + 32));
    bytes.extend_from_slice(&r.chain.to_be_bytes());
    for role in [
        r.user_proxy,
        r.proxy_impl,
        r.pmm,
        r.zero_exchange,
        r.spender,
        r.allowance_target,
        r.weth,
    ] {
        bytes.extend_from_slice(role.address.as_slice());
        bytes.extend_from_slice(role.runtime.as_slice());
    }
    keccak256(bytes)
}
fn previous() -> Option<FrozenRoles> {
    static ROLES: OnceLock<Option<FrozenRoles>> = OnceLock::new();
    *ROLES.get_or_init(|| {
        let text = crate::settings::recommendations::venue_value("tokenlon.contracts", "eth_dex")?;
        let rows = text
            .split(',')
            .map(|s| s.trim().to_owned())
            .collect::<Vec<_>>();
        let roles = FrozenRoles::from_rows(crate::settings::evm_profiles::ETHEREUM_MAINNET, &rows)?;
        (fingerprint(roles) == ORIGINAL).then_some(roles)
    })
}
/// Миграция действует при чтении старого Proof. Самостоятельный импорт Terms для новой заявки требует все поля.
pub(super) fn terms<'de, D: Deserializer<'de>>(input: D) -> Result<Terms, D::Error> {
    let mut v = Value::deserialize(input)?;
    let o = v
        .as_object_mut()
        .ok_or_else(|| D::Error::custom("Tokenlon: повреждены поля поручения"))?;
    if !o.contains_key("roles") {
        let roles = previous().ok_or_else(|| {
            D::Error::custom("Tokenlon: отсутствуют проверенные роли исторического поручения")
        })?;
        o.insert(
            "roles".into(),
            serde_json::to_value(roles).map_err(D::Error::custom)?,
        );
    }
    let t: Terms = serde_json::from_value(v).map_err(D::Error::custom)?;
    if !t.roles.verified() {
        return Err(D::Error::custom(
            "Tokenlon: исходные реализации ролей не подтверждены",
        ));
    }
    Ok(t)
}
