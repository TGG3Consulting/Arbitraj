//! Публичные сведения о предложении Native RFQ v6 и расчёт подтверждённой выплаты по событиям.
use super::super::{exec::fill::moved_logs, node::abi::IERC20};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use alloy_primitives::{Address, B256, Bytes, FixedBytes, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::abi::{IGuard, OrderV6, RFQTrade};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(in crate::venues::bsc_dex) struct Proof {
    pub pool: Address,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub sold: U256,
    pub minimum: U256,
    pub maker: Address,
    pub quote_id: FixedBytes<16>,
    pub digest: B256,
}
impl Proof {
    /// Однозначный RFQ, фактическое списание и выдача владельцу. Для модуля требуется ещё его собственный итог.
    pub(in crate::venues::bsc_dex) fn received(
        &self,
        logs: &[Log],
        module: Option<Address>,
    ) -> Option<U256> {
        let mut events = logs
            .iter()
            .filter(|l| l.address() == self.pool && l.topics() == [RFQTrade::SIGNATURE_HASH]);
        let rfq = RFQTrade::decode_log_data(&events.next()?.inner.data).ok()?;
        if events.next().is_some()
            || rfq.recipient != self.payer
            || rfq.soldToken != self.input
            || rfq.boughtToken != self.output
            || rfq.quoteId != self.quote_id
            || rfq.maker != self.maker
            || rfq.sold != self.sold
            || rfq.bought < self.minimum
            || moved_logs(logs, self.input, Some(self.payer), None) != self.sold
            || !moved_logs(logs, self.input, None, Some(self.payer)).is_zero()
            || moved_logs(logs, self.output, None, Some(self.payer)) != rfq.bought
            || !moved_logs(logs, self.output, Some(self.payer), None).is_zero()
        {
            return None;
        }
        if let Some(a) = module {
            let mut ends = logs.iter().filter(|l| {
                l.address() == a && l.topics().first() == Some(&IGuard::Settled::SIGNATURE_HASH)
            });
            let end = IGuard::Settled::decode_log_data(&ends.next()?.inner.data).ok()?;
            if ends.next().is_some()
                || end.quoteId != self.quote_id
                || end.payer != self.payer
                || end.maker != self.maker
                || end.offerDigest != self.digest
                || end.input != self.input
                || end.output != self.output
                || end.spent != rfq.sold
                || end.received != rfq.bought
                || moved_logs(logs, self.input, Some(self.payer), Some(a)) != self.sold
                || moved_logs(logs, self.input, Some(a), None) != self.sold
            {
                return None;
            }
        }
        if logs.iter().any(|l| {
            (l.address() == self.input || l.address() == self.output)
                && l.topics().first() == Some(&IERC20::Transfer::SIGNATURE_HASH)
                && IERC20::Transfer::decode_log_data(&l.inner.data).is_err()
        }) {
            return None;
        }
        Some(rfq.bought)
    }
}
pub(super) struct Firm {
    pub proof: Proof,
    pub expiry: u64,
    pub raw: Bytes,
}
pub(super) fn quantity(v: &Value) -> Option<U256> {
    let s = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => return None,
    };
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    U256::from_str_radix(&s, 10).ok()
}
fn addr(v: &Value) -> Option<Address> {
    v.as_str()?.parse().ok()
}
fn decoded(v: &Value) -> Option<Vec<u8>> {
    let s = v.as_str()?;
    hex::decode(s.strip_prefix("0x").unwrap_or(s)).ok()
}
pub(super) fn firm(
    v: &Value,
    router: Address,
    pool: Address,
    payer: Address,
    input: Address,
    output: Address,
    sold: U256,
) -> Option<Firm> {
    let [o] = v["orders"].as_array()?.as_slice() else {
        return None;
    };
    let t = &v["txRequest"];
    if v["success"] != true
        || v["router_version"].as_str() != Some("6")
        || addr(&t["target"])? != router
        || t["function"] != "tradeRFQT"
        || !quantity(&t["value"])?.is_zero()
        || v["toWrap"] != false
        || v["toUnwrap"] != false
        || !v["fallbackSwapDataArray"].is_null()
        || !decoded(&o["externalSwapCalldata"])?.is_empty()
        || o["multiHop"] != false
        || addr(&v["recipient"])? != payer
        || quantity(&v["amountIn"])? != sold
        || v["amountInOffset"] != 36
        || v["amountOutMinimumOffset"] != 68
    {
        return None;
    }
    let raw = Bytes::from(decoded(&t["calldata"])?);
    if raw.len() != 1220 || raw.get(..4)? != [0x70, 0x83, 0x52, 0x7c] {
        return None;
    }
    let frame = <(OrderV6, U256, U256)>::abi_decode_params(&raw[4..]).ok()?;
    if frame.abi_encode_params().as_slice() != &raw[4..] || !frame.1.is_zero() || !frame.2.is_zero()
    {
        return None;
    }
    let a = frame.0;
    if a.pool != pool
        || a.pool != addr(&o["pool"])?
        || a.maker.is_zero()
        || a.maker != addr(&o["signer"])?
        || a.recipient != payer
        || a.recipient != addr(&o["recipient"])?
        || a.input != input
        || a.input != addr(&o["sellerToken"])?
        || a.output != output
        || a.output != addr(&o["buyerToken"])?
        || input == output
        || a.sell != sold
        || sold.is_zero()
        || a.sell != quantity(&o["effectiveSellerTokenAmount"])?
        || a.sell != quantity(&o["sellerTokenAmount"])?
        || a.buy != quantity(&o["buyerTokenAmount"])?
        || a.buy != quantity(&v["amountOut"])?
        || a.buy != quantity(&v["amountOutBeforeFee"])?
        || a.minimum.is_zero()
        || a.minimum > a.buy
        || a.minimum != quantity(&o["amountOutMinimum"])?
        || a.expiry != quantity(&o["deadlineTimestamp"])?
        || a.nonce != quantity(&o["nonce"])?
        || a.quoteId.is_zero()
        || a.quoteId.as_slice() != decoded(&o["quoteId"])?
        || a.multiHop
        || a.mode != U256::ONE
        || !a.feeRate.is_zero()
        || a.feeRate != quantity(&o["widgetFee"]["feeRate"])?
        || a.feeRate != quantity(&v["widgetFee"]["feeRate"])?
        || a.feeRecipient != addr(&o["widgetFee"]["feeRecipient"])?
        || a.feeRecipient != addr(&v["widgetFee"]["feeRecipient"])?
        || a.signature.len() != 65
        || a.signature.as_ref() != decoded(&o["signature"])?
        || a.feeSignature.len() != 65
        || a.feeSignature.as_ref() != decoded(&o["widgetFeeSignature"])?
        || !decoded(&v["widgetFeeSignature"])?.is_empty()
    {
        return None;
    }
    let mut confidence = [U256::ZERO; 10];
    for (i, field) in [
        (0, "T"),
        (1, "N"),
        (2, "E"),
        (3, "M"),
        (4, "P"),
        (5, "O"),
        (8, "R"),
        (9, "D"),
    ] {
        confidence[i] = quantity(&o[format!("confidenceExtractedValue{field}")])?;
    }
    if confidence != a.confidence {
        return None;
    }
    Some(Firm {
        expiry: u64::try_from(a.expiry).ok()?,
        proof: Proof {
            pool,
            payer,
            input,
            output,
            sold,
            minimum: a.minimum,
            maker: a.maker,
            quote_id: a.quoteId,
            digest: keccak256(&raw),
        },
        raw,
    })
}
impl Firm {
    pub(super) fn call(&self, minimum: U256) -> Bytes {
        IGuard::executeCall {
            input: self.proof.input,
            output: self.proof.output,
            amount: self.proof.sold,
            minimum,
            deadline: U256::from(self.expiry),
            offer: self.raw.clone(),
        }
        .abi_encode()
        .into()
    }
}

// Два размера каждого направления из свежих ответов обычному кошельку CI.
#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::collections::BTreeSet;

    #[test]
    fn single_order_receipt_identifies_exact_wallet_payment_and_rejects_mixed_flows() {
        let data = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_wallets.json.gz"
        );
        let archive: Value = serde_json::from_reader(GzDecoder::new(data.as_slice())).unwrap();
        let directory: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_public.json"
        ))
        .unwrap();
        let pool = addr(&directory["roles"]["roles"][1]["address"]).unwrap();
        let mut quotes = BTreeSet::new();
        let mut directions = BTreeMap::new();
        for r in archive["observed"].as_array().unwrap() {
            assert_eq!(r["status"], "settled");
            let proof = Proof {
                pool,
                payer: addr(&archive["payer"]).unwrap(),
                input: addr(&r["source"]).unwrap(),
                output: addr(&r["target"]).unwrap(),
                sold: quantity(&r["requested"]).unwrap(),
                minimum: quantity(&r["minimum"]).unwrap(),
                maker: addr(&r["maker"]).unwrap(),
                quote_id: FixedBytes::from_slice(&decoded(&r["quoteId"]).unwrap()),
                digest: r["offer_digest"].as_str().unwrap().parse().unwrap(),
            };
            let logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
            let net = quantity(&r["net"]).unwrap();
            assert!(quotes.insert((proof.quote_id, proof.digest)));
            *directions.entry((proof.input, proof.output)).or_insert(0) += 1;
            assert_eq!(
                quantity(&r["before"][0]).unwrap() - quantity(&r["after"][0]).unwrap(),
                proof.sold
            );
            assert_eq!(
                quantity(&r["after"][1]).unwrap() - quantity(&r["before"][1]).unwrap(),
                net
            );
            assert_eq!(proof.received(&logs, None), Some(net));
            assert_eq!(proof.received(&logs, Some(proof.payer)), None);
            for extra in &logs {
                let mut many = logs.clone();
                many.push(extra.clone());
                assert_eq!(proof.received(&many, None), None);
            }
            let mut changed = proof.clone();
            changed.minimum = net + U256::ONE;
            assert_eq!(changed.received(&logs, None), None);
            changed = proof;
            changed.quote_id = FixedBytes::ZERO;
            assert_eq!(changed.received(&logs, None), None);
        }
        assert_eq!(quotes.len(), 12);
        assert_eq!(directions.len(), 6);
        assert!(directions.values().all(|n| *n == 2));
    }
    use std::collections::BTreeMap;
}
