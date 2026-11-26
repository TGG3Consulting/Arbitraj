//! Подтверждённые роли EIP-2535. Upgrades меняют адреса обработчиков, оставляя код прокси прежним.
use super::registry::Entry;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    node::{call, rpc},
};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, FixedBytes, keccak256};
use serde_json::{Value, json};

sol! {
    interface IDiamondDex {
        function facetAddress(bytes4 selector) external view returns (address);
    }
}

const FACTORY: &[&str] = &[
    "createPair(address,address)",
    "getPair(address,address)",
    "swapFee()",
    "feesEnabled()",
];
const ROUTER: &[&str] = &[
    "factory()",
    "WETH()",
    "getAmountsOut(uint256,address[])",
    "getAmountsIn(uint256,address[])",
    "swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
];

fn match_code(bytes: &Bytes, expected: B256) -> bool {
    !bytes.is_empty() && !expected.is_zero() && keccak256(bytes) == expected
}

fn confirmed(
    row: usize,
    bytes: &Bytes,
    shell: B256,
    implementation: B256,
    facet: Address,
) -> GatewayResult<bool> {
    match row {
        0 => Ok(match_code(bytes, shell)),
        1 => Ok(match_code(bytes, implementation)),
        _ => Ok(call::decode::<IDiamondDex::facetAddressCall>(bytes)? == facet),
    }
}

pub(crate) async fn check(ctx: &Ctx, id: &str, router: bool, at: &Value) -> GatewayResult<()> {
    observe(ctx, id, router, at, None).await
}

async fn observe(
    ctx: &Ctx,
    id: &str,
    router: bool,
    at: &Value,
    rate: Option<(super::v2_live_fee::Rate, u32)>,
) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Изменился код Diamond или обработчик используемого селектора",
        )
    };
    let generation = ctx.settings.generation();
    let hosts = ctx.dex_list("dex.proxy_implementations", id)?;
    let hashes = ctx.dex_list("dex.proxy_runtime_hashes", id)?;
    if hosts.len() > 2 || hashes.len() > 2 {
        return Err(bad());
    }
    let i = usize::from(router);
    let proxy = ctx.dex_addr(if router { "dex.router" } else { "dex.factory" }, id)?;
    let shell_hash = ctx
        .dex_text(
            if router {
                "dex.router_runtime_hash"
            } else {
                "dex.factory_runtime_hash"
            },
            id,
        )?
        .parse()
        .map_err(|_| bad())?;
    let facet: Address = hosts.get(i).ok_or_else(bad)?.parse().map_err(|_| bad())?;
    let facet_hash = hashes.get(i).ok_or_else(bad)?.parse().map_err(|_| bad())?;
    if facet.is_zero() || proxy.is_zero() || facet == proxy {
        return Err(bad());
    }
    let signatures = if router { ROUTER } else { FACTORY };
    let mut ops = vec![
        ("eth_getCode", json!([proxy, at])),
        ("eth_getCode", json!([facet, at])),
    ];
    for signature in signatures {
        let selector = FixedBytes::<4>::from_slice(&keccak256(signature.as_bytes())[..4]);
        let data = Bytes::from(IDiamondDex::facetAddressCall { selector }.abi_encode());
        ops.push(("eth_call", json!([{"to":proxy,"data":data},at])));
    }
    let fee_idx = ops.len();
    if let Some((rule, _)) = &rate {
        let (target, input) = rule.call();
        ops.push(("eth_call", json!([{"to":target,"data":input},at])));
    }
    let result = rpc::batch(ctx, &ops).await?;
    if result.len() != ops.len() {
        return Err(bad());
    }
    for (i, row) in result.into_iter().enumerate() {
        let raw = row?;
        let bytes: Bytes = crate::venues::bsc_dex::node::jsonrpc::decode(&raw, rpc::SOURCE)?;
        if i == fee_idx
            && let Some((rule, expected)) = &rate
        {
            if rule.decode(&bytes)? != *expected {
                return Err(ctx.err(ErrorCode::StaleData, "Изменилась комиссия пары Diamond"));
            }
            continue;
        }
        if !confirmed(i, &bytes, shell_hash, facet_hash, facet)? {
            return Err(bad());
        }
    }
    if ctx.settings.generation() != generation {
        return Err(bad());
    }
    Ok(())
}

/// Фабрика и ставка перед отправкой V5, независимо от доступности Router02.
pub(crate) async fn fresh(ctx: &Ctx, entry: &Entry, at: &Value) -> GatewayResult<()> {
    let id = entry.family.id;
    let generation = ctx.settings.generation();
    let pool = entry
        .pool
        .reference
        .own_contract()
        .ok_or_else(|| ctx.err(ErrorCode::InvalidAddress, "У пары Diamond нет адреса"))?;
    let value = entry
        .pool
        .fee_pips
        .ok_or_else(|| ctx.err(ErrorCode::StaleData, "Не подтверждена ставка Diamond"))?;
    let rule = super::v2_live_fee::Rate::configured(
        ctx,
        entry.family,
        super::v2_live_fee::Market {
            pool,
            tokens: (entry.pool.token0, entry.pool.token1),
        },
    )?;
    observe(ctx, id, false, at, Some((rule, value))).await?;
    if generation != ctx.settings.generation() {
        return Err(ctx.err(ErrorCode::StaleData, "Изменён профиль Diamond"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolValue;

    #[test]
    fn same_shell_does_not_authorize_replaced_facet_or_selector() {
        let code = Bytes::from_static(b"proxy");
        let implementation = Bytes::from_static(b"factory facet");
        let router = Bytes::from_static(b"router facet");
        let factory = Address::with_last_byte(1);
        let other = Address::with_last_byte(2);
        let check = |row, bytes: &Bytes| {
            confirmed(
                row,
                bytes,
                keccak256(&code),
                keccak256(&implementation),
                factory,
            )
        };
        assert!(check(0, &code).unwrap());
        assert!(check(1, &implementation).unwrap());
        assert!(!check(1, &router).unwrap());
        assert!(check(2, &factory.abi_encode().into()).unwrap());
        assert!(!check(3, &other.abi_encode().into()).unwrap());
        assert!(check(3, &Bytes::new()).is_err());
    }
}
