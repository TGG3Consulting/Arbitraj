//! Привязка immutable модуля к официальным фабрикам и текущей реализации селектора RouterV4.
use super::{
    direct_module, nami_batch as batch,
    pendle::{self, IMarket},
    registry::Entry,
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, quote::white_state::Head},
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::json;

const BUY: &str = "swapExactTokenForPtSimple(address,address,uint256,(address,uint256,address,address,(uint8,address,bytes,bool)))";
const BUY_SY: &str = "swapExactSyForPtSimple(address,address,uint256,uint256)";
const ORDERS: &str = "(address,uint256,((uint256,uint256,uint256,uint8,address,address,address,address,uint256,uint256,uint256,bytes),bytes,uint256)[],((uint256,uint256,uint256,uint8,address,address,address,address,uint256,uint256,uint256,bytes),bytes,uint256)[],bytes)";
fn selector(buy: bool, raw_sy: bool) -> [u8; 4] {
    let sig = match (buy, raw_sy) {
        (true, true) => BUY_SY.to_string(),
        (true, false) => BUY.to_string(),
        (false, true) => format!("swapExactPtForSy(address,address,uint256,uint256,{ORDERS})"),
        (false, false) => format!(
            "swapExactPtForToken(address,address,uint256,(address,uint256,address,address,(uint8,address,bytes,bool)),{ORDERS})"
        ),
    };
    keccak256(sig.as_bytes()).as_slice()[..4]
        .try_into()
        .expect("четыре байта селектора")
}
// RouterStorage.CoreStorage.selectorToFacet лежит через два слота после owner/pendingOwner в ERC-7201 пространстве.
fn slot(selector: [u8; 4]) -> B256 {
    let seed = U256::from_be_bytes(keccak256("pendle.routerv4.Core").0) - U256::ONE;
    let core = U256::from_be_bytes(keccak256(seed.to_be_bytes::<32>()).0) & !U256::from(255u64);
    let mut input = [0u8; 64];
    input[..4].copy_from_slice(&selector);
    input[32..].copy_from_slice(&(core + U256::from(2u64)).to_be_bytes::<32>());
    keccak256(input)
}
pub(crate) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    head: Head,
) -> GatewayResult<Address> {
    let id = e.family.id;
    let generation = ctx.settings.generation();
    let s = pendle::series(ctx, id, e.pool.reference.address())?;
    let term = s
        .terminal((e.coin, e.usdt))
        .ok_or_else(|| pendle::bad(ctx))?;
    let buy = input == term;
    if !buy && input != s.pt {
        return Err(pendle::bad(ctx));
    }
    let enabled = pendle::available(ctx, e.family, s, term, head).await?;
    if !(if buy { enabled.0 } else { enabled.1 }) {
        return Err(pendle::bad(ctx));
    }
    let roles = pendle::roles(ctx, id)?;
    let role = |key: &str| roles.get(key).copied().ok_or_else(|| pendle::bad(ctx));
    let router = role("router")?;
    let sel = selector(buy, term == s.sy);
    let name = format!("0x{}", alloy_primitives::hex::encode(sel));
    let facet = role(&name)?;
    let at = &head.selector();
    let module = ctx.dex_addr("dex.router", id)?;
    let accepted = direct_module::configured(ctx, id)?;
    let (_, _, r) = tokio::try_join!(
        router.check(ctx, at),
        facet.check(ctx, at),
        batch::read(
            ctx,
            [
                ("eth_getCode", json!([module, at])),
                ("eth_getStorageAt", json!([router.address, slot(sel), at])),
                batch::view(at, module, IMarket::routerCall {}),
                batch::view(at, module, IMarket::originalFactoryCall {}),
                batch::view(at, module, IMarket::factoryV3Call {}),
                batch::view(at, module, IMarket::factoryV4Call {}),
                batch::view(at, module, IMarket::factoryV5Call {}),
                batch::view(at, module, IMarket::factoryV6Call {}),
            ]
        )
    )?;
    let [code, selected, binding, f0, f3, f4, f5, f6] = r;
    if direct_module::matching_hash(&accepted, &code).is_none()
        || selected.as_ref() != facet.address.into_word().as_slice()
        || batch::decode(&binding, IMarket::routerCall {})? != router.address
        || [
            batch::decode(&f0, IMarket::originalFactoryCall {})?,
            batch::decode(&f3, IMarket::factoryV3Call {})?,
            batch::decode(&f4, IMarket::factoryV4Call {})?,
            batch::decode(&f5, IMarket::factoryV5Call {})?,
            batch::decode(&f6, IMarket::factoryV6Call {})?,
        ] != [
            role("marketFactory")?.address,
            role("marketFactoryV3")?.address,
            role("marketFactoryV4")?.address,
            role("marketFactoryV5")?.address,
            role("marketFactoryV6")?.address,
        ]
        || generation != ctx.settings.generation()
    {
        return Err(pendle::bad(ctx));
    }
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn four_official_selectors_have_the_observed_facet_and_runtime() {
        let data = &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/pendle_ethereum_protected_wallets.json.gz"
        )[..];
        let wallet: Value = serde_json::from_reader(flate2::read::GzDecoder::new(data)).unwrap();
        let config: Value =
            serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
        let settings = &config
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "pendle")
            .unwrap()["settings"];
        assert_eq!(
            settings["dex.module_runtime_hashes"][0],
            wallet["module"]["runtime"]
        );
        let mut remaining = wallet["module"]["bindings"].as_object().unwrap().clone();
        for buy in [true, false] {
            for raw in [true, false] {
                let sel = selector(buy, raw);
                let name = remaining
                    .keys()
                    .find(|n| keccak256(n.as_bytes()).as_slice()[..4] == sel)
                    .unwrap()
                    .clone();
                let binding = remaining.remove(&name).unwrap();
                let row = format!(
                    "0x{} {} {}",
                    alloy_primitives::hex::encode(sel),
                    binding["facet"].as_str().unwrap(),
                    binding["runtime_hash"].as_str().unwrap()
                );
                assert!(
                    settings["dex.pendle_roles"]
                        .as_array()
                        .unwrap()
                        .contains(&Value::String(row))
                );
            }
        }
        assert!(remaining.is_empty());
    }
}
