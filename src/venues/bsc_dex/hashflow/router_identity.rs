//! Проверенные на Ethereum разрешение пула и адрес Router, на том же blockHash, что и выплата.
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes};
use serde_json::{Value, json};

alloy::sol! {
    function authorizedPools(address pool) external view returns (bool);
    function router() external view returns (address);
}

pub(super) fn role_calls(target: Address, pool: Address) -> [Value; 2] {
    [
        json!({"to":target,"data":Bytes::from(authorizedPoolsCall { pool }.abi_encode())}),
        json!({"to":pool,"data":Bytes::from(routerCall {}.abi_encode())}),
    ]
}

pub(super) fn authorized(target: Address, allowed: (u64, &[u8]), actual: (u64, &[u8])) -> bool {
    allowed.0 == 1
        && actual.0 == 1
        && allowed.1 == true.abi_encode()
        && actual.1 == target.abi_encode()
}

#[cfg(test)]
mod tests {
    use super::super::sim::tag;
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn ethereum_mined_transaction_uses_the_verified_router_and_its_authorized_pool() {
        let saved: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/hashflow_direct_2026-11-26/ethereum_router.json"
        ))
        .unwrap();
        let txs: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/hashflow_direct_2026-11-26/swaps.json"
        ))
        .unwrap();
        let tx = txs
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["chain_id"] == 1)
            .unwrap();
        assert_eq!(saved["blockHash"], tx["block_hash"]);
        assert_eq!(saved["tx"], tx["transaction_hash"]);
        let target = super::super::router(1).unwrap();
        let pool: Address = tx["trade_logs"][0]["address"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            saved["router"]
                .as_str()
                .unwrap()
                .parse::<Address>()
                .unwrap(),
            target
        );
        assert_eq!(
            saved["pool"].as_str().unwrap().parse::<Address>().unwrap(),
            pool
        );
        let db = super::super::deployments().unwrap();
        assert_eq!(
            db.router_runtime[&1],
            saved["runtime"].as_str().unwrap().parse::<B256>().unwrap()
        );
        assert!(!db.router_runtime.contains_key(&56));
        let calls = role_calls(target, pool);
        let mut replies = Vec::new();
        for (call, row) in calls.iter().zip(saved["roles"].as_array().unwrap()) {
            assert_eq!(call, &row["params"][0]);
            assert_eq!(
                row["params"][1],
                tag(tx["block_hash"].as_str().unwrap().parse().unwrap())
            );
            replies.push(row["result"].as_str().unwrap().parse::<Bytes>().unwrap());
        }
        assert!(authorized(target, (1, &replies[0]), (1, &replies[1])));
        assert!(!authorized(
            target,
            (1, &false.abi_encode()),
            (1, &replies[1])
        ));
        assert!(!authorized(target, (0, &replies[0]), (1, &replies[1])));
        assert!(!authorized(target, (1, &replies[0]), (0, &replies[1])));
        assert!(!authorized(pool, (1, &replies[0]), (1, &replies[1])));
    }
}
