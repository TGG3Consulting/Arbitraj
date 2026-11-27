use super::addr;
use alloy_primitives::{B256, Bytes, U256, keccak256};
use serde_json::Value;

pub(super) fn check(a: &Value, child: &Value) {
    let tx = &a["transaction"];
    let receipt = &a["receipt"];
    assert_eq!(tx["hash"], receipt["transactionHash"]);
    assert_eq!(tx["blockHash"], child["hash"]);
    assert_eq!(receipt["blockHash"], child["hash"]);
    assert_eq!(receipt["status"], "0x1");
    assert_eq!(tx["to"], a["roles"]["dex"]);
    let call: Bytes = tx["input"].as_str().unwrap().parse().unwrap();
    assert_eq!(&call[..4], &keccak256("userCmd(uint16,bytes)")[..4]);
    assert_eq!(U256::from_be_slice(&call[4..36]), U256::ONE);
    let logs = receipt["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 1);
    let log = &logs[0];
    assert_eq!(log["address"], a["states"][0][0]["quote"]);
    let topics = log["topics"].as_array().unwrap();
    assert_eq!(
        topics[0],
        format!("{:#x}", keccak256("Transfer(address,address,uint256)"))
    );
    for (topic, address) in [(&topics[1], &tx["to"]), (&topics[2], &tx["from"])] {
        let word: B256 = topic.as_str().unwrap().parse().unwrap();
        assert_eq!(word, addr(address).into_word());
    }
    assert_eq!(
        U256::from_str_radix(log["data"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        U256::from(813541)
    );
}
