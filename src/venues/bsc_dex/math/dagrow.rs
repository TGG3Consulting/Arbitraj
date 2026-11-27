//! DAGrow Strategic Swap Pool. Комиссия в DAGR, нормализация фактического USDT и границы двух форм заявки.
use super::{
    full_math::{div_up, mul_div, mul_div_up},
    swap_cl::Swapped,
};
use crate::core::types::DagrowState;
use alloy_primitives::{Address, U256};
impl DagrowState {
    pub fn fill(&self, input: Address, exact_input: bool, n: U256) -> Option<Swapped> {
        let buy = input == self.stable;
        if (!buy && input != self.dagr)
            || n.is_zero()
            || self.stable_scale.is_zero()
            || self.reserves.contains(&U256::ZERO)
        {
            return None;
        }
        let side = usize::from(!buy);
        let base = U256::from(10_000);
        let keep = base.checked_sub(U256::from(self.fees[side]?))?;
        let (rin, rout) = (self.reserves[side], self.reserves[1 - side]);
        let (mut spend, mut out) = (n, n);
        if exact_input {
            let incoming = if buy {
                n.checked_mul(self.stable_scale)?
            } else {
                mul_div(n, keep, base)?
            };
            let gross = mul_div(incoming, rout, rin.checked_add(incoming)?)?;
            out = if buy {
                mul_div(gross, keep, base)?
            } else {
                gross / self.stable_scale
            };
        } else {
            let gross = if buy {
                mul_div_up(n, base, keep)?
            } else {
                n.checked_mul(self.stable_scale)?
            };
            let required = mul_div_up(rin, gross, rout.checked_sub(gross)?)?;
            spend = if buy {
                div_up(required, self.stable_scale)?
            } else {
                mul_div_up(required, base, keep)?
            };
        }
        if spend.is_zero() || out.is_zero() || out > self.balances[1 - side] {
            return None;
        }
        Some(Swapped {
            amount_in: spend,
            amount_out: out,
            complete: true,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_archived_quotes_agree_with_both_signed_payers_and_both_forms() {
        use serde_json::Value;
        let d: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            &include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/dagrow_ethereum_wallets.json.gz"
            )[..],
        ))
        .unwrap();
        let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        for c in d["cases"].as_array().unwrap() {
            let fee = d["fees_bps"][if c["tier"] == 0 { "ecoBps" } else { "exoBps" }]
                .as_u64()
                .unwrap() as u16;
            let mut s = DagrowState {
                pool: addr(&d["roles"]["pool"]["address"]),
                stable: addr(&d["tokens"][0]),
                dagr: addr(&d["tokens"][1]),
                payer: addr(&c["payer"]),
                fees: [Some(fee); 2],
                stable_scale: U256::from(10u64.pow(12)),
                reserves: [num(&d["reserves"][0]), num(&d["reserves"][1])],
                balances: [U256::MAX; 2],
            };
            let token = addr(&c["input"]);
            let y = s.fill(token, true, num(&c["amount"])).unwrap().amount_out;
            assert_eq!(y, num(&c["settlement"]["exact_in"]["credit"]));
            assert_eq!(
                s.fill(token, false, y).unwrap().amount_in,
                num(&c["settlement"]["exact_out"]["debit"])
            );
            s.fees[1] = None;
            assert!(s.fill(s.dagr, true, U256::ONE).is_none());
            s.balances = [U256::ZERO; 2];
            assert!(s.fill(token, false, y).is_none());
        }
    }
}
