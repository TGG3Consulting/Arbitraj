//! Извлекает уже исполненный 0x RFQ и Kipseli calldata из исходных канонических трасс.
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256};
use flate2::read::GzDecoder;
use serde_json::Value;

alloy::sol! {
    struct Permitted {address token; uint256 amount;}
    struct WitnessPermit {Permitted permitted; uint256 nonce; uint256 deadline;}
    interface ISpent {
        function swap(address tokenIn,uint256 amountIn,address tokenOut,uint256 minOut,uint256 timestamp,bytes proof) external returns(uint256);
        function RFQ(address recipient,WitnessPermit permit,address maker,bytes signature,address takerToken,uint256 maxTakerAmount) external;
        function isValidSignature(bytes32 digest,bytes signature) external view returns(bytes4);
    }
}
fn fixture(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}
fn bytes(v: &Value) -> Bytes {
    v.as_str().unwrap().parse().unwrap()
}
fn address(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn amount(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn spent_signature_and_cash_are_separate_from_fresh_access() {
    let archive = fixture(include_bytes!(
        "data/first_run_dex_2026-11-26/kipseli_zeroex_settled_traces.json.gz"
    ));
    let base = fixture(include_bytes!(
        "data/first_run_dex_2026-11-26/kipseli_base_settled.json.gz"
    ));
    let zero = fixture(include_bytes!(
        "data/first_run_dex_2026-11-26/llama_top_0x_rfq.json.gz"
    ));
    let k = &archive["kipseli"]["frame"];
    assert_eq!(k["transactionHash"], base["transaction"]["hash"]);
    assert_eq!(k["blockHash"], base["receipt"]["blockHash"]);
    assert_eq!(k["action"]["from"], base["user"]);
    assert_eq!(k["action"]["to"], base["router"]);
    let order = ISpent::swapCall::abi_decode_validate(&bytes(&k["action"]["input"])).unwrap();
    assert_eq!(order.tokenIn, address(&base["token_in"]));
    assert_eq!(order.tokenOut, address(&base["token_out"]));
    assert_eq!(order.amountIn, amount(&base["paid"]));
    assert_eq!(order.minOut, amount(&base["minimum"]));
    let (fee, rate, user, signature) =
        <(U256, U256, Address, Bytes)>::abi_decode_params(&order.proof).unwrap();
    assert_eq!(
        (fee, rate, user, signature.clone())
            .abi_encode_params()
            .as_slice(),
        order.proof.as_ref()
    );
    assert_eq!(fee, amount(&base["fee"]));
    assert_eq!(rate, amount(&base["origin_rate"]));
    assert_eq!(user, address(&base["user"]));
    assert_eq!(signature.len(), 65);
    assert_eq!(
        U256::from_be_slice(&bytes(&k["result"]["output"])),
        amount(&base["received"])
    );
    let z = &archive["zeroex"];
    let proof = ISpent::RFQCall::abi_decode_validate(&bytes(&z["settled_action"])).unwrap();
    let check = &z["signature_check"];
    assert_eq!(z["entry"]["blockHash"], zero["receipt"]["blockHash"]);
    assert_eq!(check["transactionHash"], zero["receipt"]["transactionHash"]);
    assert_eq!(proof.recipient, address(&zero["settler"]));
    assert_eq!(proof.maker, address(&zero["maker"]));
    assert_eq!(address(&check["action"]["to"]), proof.maker);
    let validation =
        ISpent::isValidSignatureCall::abi_decode_validate(&bytes(&check["action"]["input"]))
            .unwrap();
    assert_eq!(validation.signature, proof.signature);
    assert_eq!(proof.signature.len(), 65);
    let magic = ISpent::isValidSignatureCall::abi_decode_returns_validate(&bytes(
        &check["result"]["output"],
    ))
    .unwrap();
    assert_eq!(
        magic.as_slice(),
        ISpent::isValidSignatureCall::SELECTOR.as_slice()
    );
    assert_eq!(proof.permit.nonce, amount(&z["nonce"]));
    assert_eq!(proof.takerToken, address(&zero["maker_input"]["address"]));
    assert_eq!(
        proof.permit.permitted.token,
        address(&zero["maker_output"]["address"])
    );
    assert!(amount(&z["paid"]) < proof.maxTakerAmount);
    assert!(amount(&z["received"]) < proof.permit.permitted.amount);
}
