//! Сверяет активность nonce и подпись заявки с проверенным maker в указанном блоке.
use super::{
    fail,
    profile::Role,
    wire::{IRfq, Order},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call, quote::white_state::Head},
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Profile {
    pub router: Role,
    pub maker: Role,
    pub module: Option<Role>,
}
impl Profile {
    pub(super) fn target(self) -> Address {
        self.module.unwrap_or(self.maker).addr
    }
    pub(super) async fn authorize(
        self,
        ctx: &Ctx,
        o: &Order,
        sig: Bytes,
        head: Head,
    ) -> GatewayResult<B256> {
        let bad = || {
            fail(
                ctx,
                "Dexalot: домен maker, подпись, nonce или разрешение router изменились",
            )
        };
        let at = head.selector();
        let maker = self.maker.addr;
        let chain = ctx.chain_id().ok_or_else(bad)?;
        if o.maker != maker {
            return Err(bad());
        }
        let ((), allowed, v, bits) = tokio::try_join!(
            self.maker.check(ctx, &at),
            call::canonical(ctx, &at, self.router.addr, IRfq::getAllowedRFQsCall {}),
            call::canonical(ctx, &at, maker, IRfq::VERSIONCall {}),
            call::canonical(
                ctx,
                &at,
                maker,
                IRfq::completedSwapsCall {
                    bucket: o.nonceAndMeta >> 8
                }
            )
        )?;
        if !allowed.contains(&maker) || bits.bit((o.nonceAndMeta & U256::from(255)).to::<usize>()) {
            return Err(bad());
        }
        let version = match v.as_slice().split(|&c| c == 0).next() {
            Some(b"1.2.8") => "1",
            Some(b"2.1.1") => "2",
            _ => return Err(bad()),
        };
        let digest = o.digest(chain, version);
        let valid = call::canonical(
            ctx,
            &at,
            maker,
            IRfq::isValidSignatureCall {
                digest,
                signature: sig,
            },
        )
        .await?;
        if valid.as_slice() != [0x16, 0x26, 0xba, 0x7e] {
            return Err(bad());
        }
        if self.module.is_some()
            && !call::canonical(
                ctx,
                &at,
                maker,
                IRfq::hasRoleCall {
                    role: keccak256("TRUSTED_FORWARDER_ROLE"),
                    grantee: self.router.addr,
                },
            )
            .await?
        {
            return Err(bad());
        }
        Ok(digest)
    }
}
