//! Роли опубликованного Reactor, его EIP-1967 реализации, Quoter и собственного модуля.
use super::*;
use crate::{settings::scope::Scope, venues::bsc_dex::pools::solidly_classic_factory as binding};
use alloy_primitives::keccak256;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct Role {
    pub address: Address,
    pub hash: B256,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct Profile {
    pub chain: u64,
    pub reactor: Role,
    pub implementation: Role,
    pub quoter: Role,
    pub module: Role,
}
impl Profile {
    pub(super) fn read(ctx: &Ctx) -> Option<Self> {
        let chain = ctx.chain_id()?;
        let rows = ctx
            .settings
            .get_exact("gladius.roles", &Scope::Venue(ctx.place.id.to_string()))?;
        let mut entries = std::collections::BTreeMap::new();
        let mut accepted = None;
        for row in rows.as_array()? {
            let mut words = row.as_str()?.split_whitespace();
            let key = words.next()?;
            let val = words.next()?;
            if key == "network" {
                if accepted.replace(val.parse::<u64>().ok()?).is_some() {
                    return None;
                }
            } else {
                let role = Role {
                    address: val.parse().ok()?,
                    hash: words.next()?.parse().ok()?,
                };
                if role.address.is_zero()
                    || role.hash.is_zero()
                    || entries.insert(key, role).is_some()
                {
                    return None;
                }
            }
            if words.next().is_some() {
                return None;
            }
        }
        if accepted != Some(chain) || entries.len() != 4 {
            return None;
        }
        Some(Self {
            chain,
            reactor: entries.remove("reactor")?,
            implementation: entries.remove("implementation")?,
            quoter: entries.remove("quoter")?,
            module: entries.remove("module")?,
        })
    }
    pub(super) async fn verify(&self, ctx: &Ctx, h: white_state::Head) -> GatewayResult<()> {
        let tag = h.selector();
        let get = async |method: &str| -> GatewayResult<B256> {
            let bytes:Bytes=rpc::call(ctx,"eth_call",json!([{"to":self.module.address,"input":Bytes::copy_from_slice(&keccak256(method.as_bytes())[..4])},tag])).await?;
            (bytes.len() == 32)
                .then(|| B256::from_slice(&bytes))
                .ok_or_else(|| no(ctx, "Gladius: модуль не подтвердил роли"))
        };
        let (_, _, _, target, hash) = tokio::try_join!(
            binding::bind(
                ctx,
                self.reactor.address,
                self.reactor.hash,
                Some((self.implementation.address, self.implementation.hash)),
                &tag
            ),
            binding::plain(ctx, self.quoter.address, self.quoter.hash, &tag),
            binding::plain(ctx, self.module.address, self.module.hash, &tag),
            get("reactor()"),
            get("reactorHash()")
        )?;
        if target != self.reactor.address.into_word() || hash != self.reactor.hash {
            return Err(no(ctx, "Gladius: у модуля другой Reactor"));
        }
        Ok(())
    }
}
