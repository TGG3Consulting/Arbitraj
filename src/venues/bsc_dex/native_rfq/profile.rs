//! Адреса Native RFQ, runtime и четыре неизменяемые роли защитного модуля.
use super::{Ctx, bad, rpc, white_state};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    settings::scope::Scope,
};
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};
use std::collections::BTreeMap;
const PROFILE: &str = "native_rfq.roles";

#[derive(Clone, Copy)]
pub(super) struct Role {
    pub(super) address: Address,
    pub(super) hash: B256,
}
pub(super) struct Profile {
    pub(super) chain: u64,
    pub(super) slug: String,
    pub(super) router: Role,
    pub(super) pool: Role,
    pub(super) module: Option<Role>,
}
impl Profile {
    fn parse(rows: &Value) -> Option<Self> {
        let mut roles = BTreeMap::new();
        let mut network = None;
        for row in rows.as_array()? {
            let mut fields = row.as_str()?.split_whitespace();
            let name = fields.next()?;
            let first = fields.next()?;
            let second = fields.next()?;
            if fields.next().is_some() {
                return None;
            }
            if name == "network" {
                if network.is_some()
                    || second.is_empty()
                    || !second
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                {
                    return None;
                }
                network = Some((first.parse::<u64>().ok()?, second.to_owned()));
            } else {
                let role = Role {
                    address: first.parse().ok()?,
                    hash: second.parse().ok()?,
                };
                if role.address.is_zero()
                    || role.hash.is_zero()
                    || !matches!(name, "router" | "pool" | "module")
                    || roles.insert(name, role).is_some()
                {
                    return None;
                }
            }
        }
        let (chain, slug) = network?;
        let router = roles.remove("router")?;
        let pool = roles.remove("pool")?;
        let module = roles.remove("module");
        if chain == 0
            || router.address == pool.address
            || module.is_some_and(|m| m.address == pool.address || m.address == router.address)
        {
            return None;
        }
        Some(Self {
            chain,
            slug,
            router,
            pool,
            module,
        })
    }
    pub(super) fn read(ctx: &Ctx) -> GatewayResult<Self> {
        let scope = Scope::Venue(ctx.place.id.to_string());
        let rows = ctx.settings.get_exact(PROFILE, &scope);
        rows.as_ref().and_then(Self::parse).filter(|p|Some(p.chain)==ctx.chain_id())
          .ok_or_else(||ctx.not_set(PROFILE,"Укажите сеть, router и pool с хешами кода в настройках Native RFQ этой площадки"))
    }
    pub(super) async fn verify(&self, ctx: &Ctx, head: white_state::Head) -> GatewayResult<()> {
        let verify = async |r: Role| {
            let code: Bytes =
                rpc::call(ctx, "eth_getCode", json!([r.address, head.selector()])).await?;
            if code.is_empty() || keccak256(&code) != r.hash {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    "Изменился код одного из настроенных контрактов Native RFQ",
                ));
            }
            Ok(())
        };
        let optional = async {
            if let Some(m) = self.module {
                verify(m).await?;
                let read = async |method: &str| -> GatewayResult<B256> {
                    let selector = Bytes::copy_from_slice(&keccak256(method.as_bytes())[..4]);
                    let result: Bytes = rpc::call(
                        ctx,
                        "eth_call",
                        json!([{"to":m.address,"input":selector},head.selector()]),
                    )
                    .await?;
                    if result.len() != 32 {
                        return Err(bad(ctx, "Неполное подтверждение ролей модуля Native"));
                    }
                    Ok(B256::from_slice(&result))
                };
                let (router, pool, router_hash, pool_hash) = tokio::try_join!(
                    read("router()"),
                    read("pool()"),
                    read("routerHash()"),
                    read("poolHash()")
                )?;
                if router != self.router.address.into_word()
                    || pool != self.pool.address.into_word()
                    || router_hash != self.router.hash
                    || pool_hash != self.pool.hash
                {
                    return Err(bad(
                        ctx,
                        "Модуль Native привязан к другому Router или RFQPool",
                    ));
                }
            }
            Ok(())
        };
        tokio::try_join!(verify(self.router), verify(self.pool), optional)?;
        Ok(())
    }
}
