use super::{
    fail,
    wire::{IRfq, IWallet, Order},
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx, exec::carbon_binding::Code, node::call, pools::solidly_classic_factory,
        quote::white_state::Head, send::receipt::Receipt,
    },
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Role {
    pub addr: Address,
    pub hash: B256,
    pub proxy: Option<(Address, B256)>,
}
impl Role {
    fn parse(parts: &[&str]) -> Option<Self> {
        let role = |a: &str, h: &str| {
            let addr: Address = a.parse().ok()?;
            let hash: B256 = h.parse().ok()?;
            (!addr.is_zero() && !hash.is_zero() && hash != keccak256([])).then_some((addr, hash))
        };
        let (&a, &h, rest) = (parts.first()?, parts.get(1)?, parts.get(2..)?);
        let (addr, hash) = role(a, h)?;
        let proxy = match rest {
            [] => None,
            [i, c] => Some(role(i, c)?),
            _ => return None,
        };
        Some(Self { addr, hash, proxy })
    }
    pub(super) async fn check(self, ctx: &Ctx, at: &Value) -> GatewayResult<()> {
        solidly_classic_factory::bind(ctx, self.addr, self.hash, self.proxy, at).await
    }
    pub(super) async fn included(self, ctx: &Ctx, tx: B256, r: &Receipt) -> GatewayResult<()> {
        if let Some((implementation, runtime)) = self.proxy {
            Code {
                shell: self.hash,
                implementation,
                runtime,
            }
            .at_transaction(ctx, self.addr, tx, r, None)
            .await
        } else {
            self.check(
                ctx,
                &json!({"blockHash":r.block_hash,"requireCanonical":true}),
            )
            .await
        }
    }
}
pub(super) struct Config {
    pub router: Role,
    pub makers: Vec<Role>,
    pub modules: Vec<Role>,
}
impl Config {
    pub(super) fn parse(rows: &[String]) -> Option<Self> {
        let mut router = None;
        let mut makers = Vec::new();
        let mut modules = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for s in rows {
            let xs: Vec<_> = s.split_whitespace().collect();
            let (kind, rest) = xs.split_first()?;
            let r = Role::parse(rest)?;
            if !seen.insert(r.addr) {
                return None;
            }
            match *kind {
                "router" if r.proxy.is_some() && router.is_none() => router = Some(r),
                "maker" => makers.push(r),
                "module" if r.proxy.is_none() => modules.push(r),
                _ => return None,
            }
        }
        (!makers.is_empty()).then_some(Self {
            router: router?,
            makers,
            modules,
        })
    }
    pub(super) fn read(ctx: &Ctx) -> GatewayResult<Self> {
        Self::parse(&ctx.settings.list("dexalot.roles", ctx.at())?).ok_or_else(|| {
            fail(
                ctx,
                "Dexalot: не подтверждены настроенные router, maker и runtime",
            )
        })
    }
    pub(super) async fn ready(
        &self,
        ctx: &Ctx,
        owner: Address,
        module: Option<Role>,
        h: Head,
    ) -> GatewayResult<()> {
        let at = h.selector();
        self.router.check(ctx, &at).await?;
        if let Some(m) = module {
            let ((), who, router, hash, weth) = tokio::try_join!(
                m.check(ctx, &at),
                call::canonical(ctx, &at, m.addr, IWallet::ownerCall {}),
                call::canonical(ctx, &at, m.addr, IWallet::routerCall {}),
                call::canonical(ctx, &at, m.addr, IWallet::routerHashCall {}),
                call::canonical(ctx, &at, m.addr, IWallet::wrappedCall {})
            )?;
            if (
                owner,
                self.router.addr,
                self.router.hash,
                ctx.addr("dex.native_wrapped")?,
            ) != (who, router, hash, weth)
            {
                return Err(fail(
                    ctx,
                    "Dexalot: исходные роли защищённого кошелька изменились",
                ));
            }
        }
        Ok(())
    }
}
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
