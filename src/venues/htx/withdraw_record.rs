//! The legacy reference contained only id/client-id. New references also keep
//! the exact submitted terms across restarts, without changing deposit records.
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{errors, num};
use crate::gateway::errors::{ErrorCode, GatewayResult};

#[derive(Serialize, Deserialize)]
pub(super) struct Submitted {
    pub currency: String,
    pub chain: String,
    pub wire: Decimal,
    pub fee: Decimal,
}

impl Submitted {
    /// HTX Spot API, Wallet → Create a Withdraw Request.
    pub fn body(&self, address: &str, cid: &str) -> Value {
        json!({
            "address": address,
            "currency": self.currency,
            "amount": self.wire.normalize().to_string(),
            "fee": self.fee.normalize().to_string(),
            "chain": self.chain,
            "client-order-id": cid,
        })
    }
    /// HTX Spot API, Wallet → Query withdrawal order by client order id.
    pub fn check_identity(&self, rec: &Value, id: &str, cid: &str) -> GatewayResult<()> {
        if num::text(rec, "id")? != id
            || num::text(rec, "client-order-id")? != cid
            || num::text(rec, "type")? != "withdraw"
            || !num::text(rec, "currency")?.eq_ignore_ascii_case(&self.currency)
            || !num::text(rec, "chain")?.eq_ignore_ascii_case(&self.chain)
        {
            return Err(errors::bad(
                "запись HTX не совпадает с принятой заявкой вывода",
                "",
            ));
        }
        Ok(())
    }

    /// HTX withdrawal records expose amount and fee separately. Compare against
    /// the actual submitted terms before normalizing either observed amount representation.
    pub fn debit_from_record(&self, rec: &Value) -> GatewayResult<Decimal> {
        let amount = num::dec(rec, "amount")?;
        let fee = num::dec(rec, "fee")?;
        if self.wire <= Decimal::ZERO || fee < Decimal::ZERO {
            return Err(errors::bad(
                "комиссия или сумма выписки HTX не совпадает с заявкой",
                "",
            ));
        }
        if fee != self.fee
            && (num::text_opt(rec, "state").as_deref() != Some("confirmed")
                || num::text_opt(rec, "tx-hash").is_none_or(|h| h.trim().is_empty()))
        {
            return Err(errors::bad(
                "изменённая комиссия вывода HTX ещё не подтверждена хешем",
                "",
            ));
        }
        let debit = self
            .wire
            .checked_add(fee)
            .ok_or_else(|| errors::bad("слишком большая сумма вывода HTX", ""))?;
        if amount != self.wire && amount != debit {
            return Err(errors::bad("сумма выписки HTX не совпадает с заявкой", ""));
        }
        Ok(debit)
    }
}

/// Resolve an ambiguous POST only by reading the original client-order-id.
pub(super) async fn accepted(
    result: GatewayResult<String>,
    lookup: impl std::future::Future<Output = Option<String>>,
) -> GatewayResult<String> {
    match result {
        Err(e) if e.code == ErrorCode::OutcomeUnknown => lookup.await.ok_or(e),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::errors::GatewayError;

    fn submitted() -> Submitted {
        Submitted {
            currency: "xter".into(),
            chain: "xter".into(),
            wire: "10008.425856".parse().unwrap(),
            fee: "31.454144".parse().unwrap(),
        }
    }

    #[test]
    fn binds_withdrawal_facts_to_original_request() {
        let s = submitted();
        let body = s.body("target", "ours");
        assert_eq!(body["amount"], "10008.425856");
        assert_eq!(body["fee"], "31.454144");
        let mut rec = json!({"id":17,"client-order-id":"ours","type":"withdraw","currency":"xter",
            "chain":"xter","amount":"10008.425856","fee":"31.454144"});
        s.check_identity(&rec, "17", "ours").unwrap();
        let debit = "10039.88".parse::<Decimal>().unwrap();
        assert_eq!(s.debit_from_record(&rec).unwrap(), debit);
        rec["amount"] = json!("10039.88");
        assert_eq!(s.debit_from_record(&rec).unwrap(), debit);
        rec["fee"] = json!("20");
        assert!(s.debit_from_record(&rec).is_err());
        rec["amount"] = json!("10008.425856");
        rec["state"] = json!("confirmed");
        assert!(s.debit_from_record(&rec).is_err());
        rec["tx-hash"] = json!("actual-withdrawal");
        let changed_debit = "10028.425856".parse::<Decimal>().unwrap();
        assert_eq!(s.debit_from_record(&rec).unwrap(), changed_debit);
        rec["amount"] = json!("10028.425856");
        assert_eq!(s.debit_from_record(&rec).unwrap(), changed_debit);
        rec["type"] = json!("deposit");
        assert!(s.check_identity(&rec, "17", "ours").is_err());
        rec["type"] = json!("withdraw");
        assert!(s.check_identity(&rec, "16", "ours").is_err());
        assert!(s.check_identity(&rec, "17", "other").is_err());
        rec["chain"] = json!("other");
        assert!(s.check_identity(&rec, "17", "ours").is_err());
    }

    #[tokio::test]
    async fn ambiguous_result_remains_unknown_without_a_matching_withdrawal() {
        let unknown = || GatewayError::new(ErrorCode::OutcomeUnknown, None, "timeout");
        assert_eq!(
            accepted(Err(unknown()), async { None })
                .await
                .unwrap_err()
                .code,
            ErrorCode::OutcomeUnknown
        );
        assert_eq!(
            accepted(Err(unknown()), async { Some("17".into()) })
                .await
                .unwrap(),
            "17"
        );
        assert_eq!(
            accepted(Ok("19".into()), async {
                panic!("accepted POST must not be looked up")
            })
            .await
            .unwrap(),
            "19"
        );
        let refused = GatewayError::new(ErrorCode::WithdrawRejected, None, "insufficient");
        assert_eq!(
            accepted(Err(refused), async { panic!("explicit rejection") })
                .await
                .unwrap_err()
                .code,
            ErrorCode::WithdrawRejected
        );
    }
}
