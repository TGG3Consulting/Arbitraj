//! Доверенные роли принадлежат конкретным книге, сети и размещению владельца.
use super::*;
use crate::settings::scope::Scope;
use crate::venues::bsc_dex::pools::solidly_classic_factory as binding;
use alloy_primitives::keccak256;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub(super) struct Profile {
    pub chain: u64,
    pub version: u8,
    pub book: Address,
    pub book_hash: B256,
    pub module: Address,
    pub module_hash: B256,
}
#[derive(Clone)]
pub(super) struct Source {
    pub profile: Profile,
    pub origin: Origin,
}
#[derive(Clone)]
pub(super) enum Origin {
    Graph(reqwest::Url),
    Explorer(reqwest::Url),
    Rpc(u64),
}
impl Origin {
    pub(super) fn parse(version: u8, kind: &str, endpoint: &str) -> Option<Self> {
        match kind {
            "rpc" if matches!(version, 3..=7) && endpoint.bytes().all(|b| b.is_ascii_digit()) => {
                let block = endpoint.parse::<u64>().ok()?;
                (block > 0).then_some(Self::Rpc(block))
            }
            "graphql" | "blockscout" => {
                let url = reqwest::Url::parse(endpoint).ok()?;
                if url.scheme() != "https"
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.fragment().is_some()
                    || url.query().is_some()
                {
                    return None;
                }
                Some(if kind == "graphql" {
                    Self::Graph(url)
                } else {
                    Self::Explorer(url)
                })
            }
            _ => None,
        }
    }
}
impl Profile {
    pub(super) fn list(ctx: &Ctx) -> Option<Vec<Source>> {
        let chain = ctx.chain_id()?;
        let setting = ctx
            .settings
            .get_exact("raindex.books", &Scope::Venue(ctx.place.id.to_string()))?;
        let mut sources = vec![];
        let mut ids = std::collections::HashSet::new();
        for row in setting.as_array()? {
            let parts = row.as_str()?.split_whitespace().collect::<Vec<_>>();
            let [
                network,
                version,
                book,
                book_hash,
                module,
                module_hash,
                kind,
                url,
            ] = parts.as_slice()
            else {
                return None;
            };
            let p = Self {
                chain: network.parse().ok()?,
                version: version.parse().ok()?,
                book: book.parse().ok()?,
                book_hash: book_hash.parse().ok()?,
                module: module.parse().ok()?,
                module_hash: module_hash.parse().ok()?,
            };
            if p.chain != chain
                || !matches!(p.version, 3..=7)
                || p.book.is_zero()
                || p.module.is_zero()
                || p.book == p.module
                || p.book_hash.is_zero()
                || p.module_hash.is_zero()
                || !ids.insert(p.book)
            {
                return None;
            }
            let origin = Origin::parse(p.version, kind, url)?;
            sources.push(Source { profile: p, origin });
        }
        Some(sources)
    }
    pub(super) async fn verify(&self, ctx: &Ctx, h: white_state::Head) -> GatewayResult<()> {
        let tag = h.selector();
        let bad = || {
            no(
                ctx,
                "Raindex: не подтверждена привязка модуля к книге и её ABI",
            )
        };
        let word = async |name: &str| -> GatewayResult<B256> {
            let bytes:Bytes=rpc::call(ctx,"eth_call",json!([{"to":self.module,"input":Bytes::copy_from_slice(&keccak256(name)[..4])},tag])).await?;
            (bytes.len() == 32)
                .then(|| B256::from_slice(&bytes))
                .ok_or_else(bad)
        };
        let (_, _, b, code, sel) = tokio::try_join!(
            binding::plain(ctx, self.book, self.book_hash, &tag),
            binding::plain(ctx, self.module, self.module_hash, &tag),
            word("book()"),
            word("bookCodeHash()"),
            word("takeSelector()")
        )?;
        use alloy::sol_types::SolCall;
        let selector = match self.version {
            3 => wire::takeOrdersCall::SELECTOR,
            4 => wire::takeOrders2Call::SELECTOR,
            5 => wire::takeOrders3Call::SELECTOR,
            6 => wire::takeOrders4Call::SELECTOR,
            7 => wire::takeOrders5Call::SELECTOR,
            _ => return Err(bad()),
        };
        if ctx.chain_id() != Some(self.chain)
            || b != self.book.into_word()
            || code != self.book_hash
            || sel[..4] != selector
            || sel[4..].iter().any(|x| *x != 0)
        {
            return Err(bad());
        }
        Ok(())
    }
}
