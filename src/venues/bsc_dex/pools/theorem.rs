//! Привязки Theorem на Ethereum. QuoterV2 возвращает фактически израсходованный вход во втором слове.
//! ABI: [IQuoterV2.sol, Algebra Integral v1.2.3](https://github.com/cryptoalgebra/Algebra/blob/1ff43e11407872f7c09b001cdeee1a59d2c334ec/src/periphery/contracts/interfaces/IQuoterV2.sol).
//! Адрес квотера взят из [клиента Theorem](https://app.theorem.markets/assets/index-V4IkL7Qa.js);
//! фактические ответы контракта на H26494988 записаны в theorem_ethereum.json.gz.
//! getPool(): [исходник AlgebraUpgradeablePlugin](https://eth.blockscout.com/address/0xaaa81db3cb943c20b032d16d9cbf471a3757931d?tab=contract).
//! implementation() и beacon slot: [EIP-1967](https://eips.ethereum.org/EIPS/eip-1967#beacon-contract-address).
//! router(): исходный public immutable router в contracts/AlgebraRouterWallet.sol этого репозитория.
use super::{families::Family, nami_batch as batch, solidly_classic_factory::plain};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256, b256};
use serde_json::{Value, json};

alloy::sol! {
    interface ITheorem {
        struct Query { address tokenIn; address tokenOut; address deployer; uint256 amount; uint160 limit; }
        function quoteExactInputSingle(Query calldata query) external returns (
            uint256 out, uint256 spent, uint160 price, uint32 ticks, uint256 gasUsed, uint16 fee);
        function router() external view returns (address);
        function getPool() external view returns (address);
        function implementation() external view returns (address);
    }
}
const BEACON: B256 = b256!("a3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50");

pub(super) fn calldata(a:Address,b:Address,amount:U256,deployer:Address)->Bytes {
    ITheorem::quoteExactInputSingleCall{query:ITheorem::Query{tokenIn:a,tokenOut:b,deployer,amount,limit:Default::default()}}
        .abi_encode().into()
}

pub(super) fn output(raw:&[u8],requested:U256)->Option<U256>{
    let q=ITheorem::quoteExactInputSingleCall::abi_decode_returns_validate(raw).ok()?;
    (q.spent==requested && !q.out.is_zero()).then_some(q.out)
}

pub(super) async fn plugin(ctx:&Ctx,f:&Family,pool:Address,plug:Address,at:&Value)->GatewayResult<()> {
    let bad=||ctx.err(ErrorCode::NotApplicable,"Theorem: изменились плагин пула, его маяк или реализация");
    let bound=ITheorem::getPoolCall{};
    let [slot,owner]=batch::read(ctx,[
        ("eth_getStorageAt",json!([plug,BEACON,at])),
        batch::view(at,plug,bound.clone()),
    ]).await?;
    let slot: [u8;32]=slot[..].try_into().map_err(|_|bad())?;
    if batch::decode(&owner,bound)?!=pool || slot[..12]!=[0;12] {return Err(bad())}
    let beacon=Address::from_slice(&slot[12..]);
    if beacon.is_zero(){return Ok(())}
    let mut selected=None;
    for row in ctx.dex_list("dex.plugin_beacon_bindings", f.id)? {
        let mut fields=row.split_whitespace();
        let mut next=||fields.next().ok_or_else(bad);
        let address:Address=next()?.parse().map_err(|_|bad())?;
        let code:B256=next()?.parse().map_err(|_|bad())?;
        let implementation:Address=next()?.parse().map_err(|_|bad())?;
        let runtime:B256=next()?.parse().map_err(|_|bad())?;
        if fields.next().is_some() || address.is_zero() || implementation.is_zero() || code.is_zero() || runtime.is_zero(){return Err(bad())}
        if address==beacon && selected.replace((code,implementation,runtime)).is_some(){return Err(bad())}
    }
    let (code,implementation,runtime)=selected.ok_or_else(bad)?;
    let getter=ITheorem::implementationCall{};
    let ((),(),[value])=tokio::try_join!(
        plain(ctx,beacon,code,at),plain(ctx,implementation,runtime,at),
        batch::read(ctx,[batch::view(at,beacon,getter.clone())])
    )?;
    if batch::decode(&value,getter)?!=implementation{return Err(bad())}
    Ok(())
}

pub(super) async fn wallet(ctx:&Ctx,e:&super::registry::Entry,module:Address,at:&Value)->GatewayResult<()> {
    let original=ctx.dex_addr("dex.router",e.family.id)?;
    let runtime=super::safeswap_trade::hash(ctx,e.family.id,"dex.router_runtime_hash")?;
    let getter=ITheorem::routerCall{};
    let ((),(),[value])=tokio::try_join!(super::cypher::router(ctx,e,original,at),plain(ctx,original,runtime,at),
        batch::read(ctx,[batch::view(at,module,getter.clone())]))?;
    if batch::decode(&value,getter)?!=original {
        return Err(ctx.err(ErrorCode::NotApplicable,"Theorem: модуль обслуживает другой Router"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    #[test]
    fn all_full_and_partial_quotes_use_the_actual_input_field() {
        let archive: Value = serde_json::from_reader(GzDecoder::new(
            include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/theorem_ethereum.json.gz").as_slice()
        )).unwrap();
        let mut checked = 0;
        for row in archive["wallets"]["cases"].as_array().unwrap() {
            for side in ["buy", "sell"] {
                let q = &row[side]["quote"];
                if q["status"] != "0x1" { continue }
                let raw: Bytes = q["data"].as_str().unwrap().parse().unwrap();
                let spent: U256 = q["spent"].as_str().unwrap().parse().unwrap();
                let received: U256 = q["received"].as_str().unwrap().parse().unwrap();
                assert_eq!(output(&raw, spent), (!received.is_zero()).then_some(received));
                assert!(output(&raw, spent + U256::ONE).is_none());
                assert!(output(&raw[..32], spent).is_none());
                checked += 1;
            }
        }
        assert!(checked >= 50);
    }
}
