//! Проверка исходного плательщика, двух валют, лимита и назначенного контракта до симуляции.
use super::{Ticket, zeroex as market};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

alloy::sol! {
    struct AllowedSlippage {address recipient; address buyToken; uint256 minAmountOut;}
    interface ISettler {function execute(AllowedSlippage slippage, bytes[] actions, bytes32 zid) external payable returns(bool);}
    interface IKipseli {function swap(address tokenIn,uint256 amountIn,address tokenOut,uint256 minOut,uint256 timestamp,bytes verification) external returns(uint256);}
    interface IAllowanceHolder {function exec(address operator,address token,uint256 amount,address target,bytes data) external payable returns(bytes);}
}
pub(super) fn kipseli(t: &Ticket, r: &Value) -> Option<(Bytes, Value)> {
    let proof = r["verificationData"].as_str()?.parse::<Bytes>().ok()?;
    let (fee, rate, user, signature) =
        <(U256, U256, Address, Bytes)>::abi_decode_params(&proof).ok()?;
    if fee != U256::from(t.fee?)
        || user != t.owner
        || signature.len() != 65
        || !matches!(signature[64], 27 | 28)
        || (fee, rate, user, signature).abi_encode_params().as_slice() != proof.as_ref()
    {
        return None;
    }
    let stamp = r["timestamp"].as_u64().filter(|v| *v > 0)?;
    let data: Bytes = IKipseli::swapCall {
        tokenIn: t.input,
        amountIn: t.amount,
        tokenOut: t.output,
        minOut: t.minimum,
        timestamp: U256::from(stamp),
        verification: proof,
    }
    .abi_encode()
    .into();
    if !r["calldata"].is_null() && r["calldata"].as_str()?.parse::<Bytes>().ok()? != data {
        return None;
    }
    Some((
        data,
        json!({"service_timestamp":stamp,"fee_tenths_bps":t.fee,"origin_rate_raw":rate.to_string()}),
    ))
}
pub(super) fn settler(t: &Ticket, data: &Bytes) -> Option<Address> {
    let a = IAllowanceHolder::execCall::abi_decode_validate(data).ok()?;
    let order = ISettler::executeCall::abi_decode_validate(&a.data).ok()?;
    if order.slippage.recipient != t.owner
        || order.slippage.buyToken != t.output
        || order.actions.is_empty()
        || order.slippage.minAmountOut < t.minimum
    {
        return None;
    }
    (a.token == t.input
        && a.amount == t.amount
        && !a.data.is_empty()
        && !a.target.is_zero()
        && a.operator == a.target)
        .then_some(a.target)
}
pub(super) fn zeroex(t: &Ticket, r: &Value) -> Option<(Bytes, Value)> {
    let (_, mut details) = market(r, t.input, t.output, t.amount)?;
    if r["minBuyAmount"].as_str()?.parse::<U256>().ok()? < t.minimum {
        return None;
    }
    let tx = &r["transaction"];
    if tx["to"].as_str()?.parse::<Address>().ok()? != t.target
        || tx["value"].as_str()?.parse::<U256>().ok()? != U256::ZERO
    {
        return None;
    }
    if !r["allowanceTarget"].is_null()
        && r["allowanceTarget"].as_str()?.parse::<Address>().ok()? != t.target
    {
        return None;
    }
    if !r["issues"]["allowance"].is_null()
        && r["issues"]["allowance"]["spender"]
            .as_str()?
            .parse::<Address>()
            .ok()?
            != t.target
    {
        return None;
    }
    let data = tx["data"].as_str()?.parse::<Bytes>().ok()?;
    settler(t, &data)?;
    details["api_minimum_raw"] = r["minBuyAmount"].clone();
    Some((data, details))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use flate2::read::GzDecoder;
    #[test]
    fn canonical_holder_call_binds_real_spender_payee_output_and_floor() {
        let f:Value=serde_json::from_reader(GzDecoder::new(&include_bytes!(
            "../../../../../tests/data/first_run_dex_2026-11-26/kipseli_zeroex_settled_traces.json.gz")[..])).unwrap();
        let row = &f["zeroex"]["holder"]["action"];
        let raw = row["input"].as_str().unwrap().parse::<Bytes>().unwrap();
        let outer = IAllowanceHolder::execCall::abi_decode_validate(&raw).unwrap();
        let inner = ISettler::executeCall::abi_decode_validate(&outer.data).unwrap();
        let mut t = Ticket {
            owner: row["from"].as_str().unwrap().parse().unwrap(),
            input: outer.token,
            output: inner.slippage.buyToken,
            amount: outer.amount,
            minimum: inner.slippage.minAmountOut,
            fee: None,
            target: row["to"].as_str().unwrap().parse().unwrap(),
            runtime: B256::ZERO,
            generation: 0,
        };
        assert_eq!(settler(&t, &raw), Some(outer.target));
        t.minimum += U256::ONE;
        assert_eq!(settler(&t, &raw), None);
        t.minimum -= U256::ONE;
        t.owner = outer.target;
        assert_eq!(settler(&t, &raw), None);
    }
    #[test]
    fn kipseli_rebuilds_the_original_settled_authorization() {
        let source: Value = serde_json::from_reader(GzDecoder::new(
            &include_bytes!(
                "../../../../../tests/data/first_run_dex_2026-11-26/kipseli_base_settled.json.gz"
            )[..],
        ))
        .unwrap();
        let addr = |k: &str| source[k].as_str().unwrap().parse::<Address>().unwrap();
        let val = |k: &str| source[k].as_str().unwrap().parse::<U256>().unwrap();
        let mut t = Ticket {
            owner: addr("user"),
            input: addr("token_in"),
            output: addr("token_out"),
            amount: val("paid"),
            minimum: val("minimum"),
            fee: Some(0),
            target: addr("router"),
            runtime: B256::ZERO,
            generation: 0,
        };
        let original = source["settled_action"]
            .as_str()
            .unwrap()
            .parse::<Bytes>()
            .unwrap();
        let o = IKipseli::swapCall::abi_decode_validate(&original).unwrap();
        let r = json!({"timestamp":source["request_timestamp"],"verificationData":o.verification,"calldata":original});
        assert_eq!(kipseli(&t, &r).unwrap().0, original);
        t.minimum += U256::ONE;
        assert!(kipseli(&t, &r).is_none());
        t.minimum -= U256::ONE;
        t.fee = Some(1);
        assert!(kipseli(&t, &r).is_none());
        t.fee = Some(0);
        t.owner = t.target;
        assert!(kipseli(&t, &r).is_none());
    }
}
