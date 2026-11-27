//! Проверка получателей входного ERC20 в симуляции. Баланс модуля, расход плательщика
//! и учёт хранилища относятся к разным адресам. Списание при сжигании к приходу не относится.
use super::{Address, Call, CallResult, IMoney, Order, U256, Wire};
use crate::venues::bsc_dex::check::simulate::SimLog;
use alloy::sol_types::SolCall;
use std::borrow::Cow;
#[derive(Clone)]
pub(in crate::venues::bsc_dex) struct BurnFunding {
    pub credited: Vec<(Address, U256)>,
    pub burner: Address,
    pub amount: U256,
}

pub(super) struct Inventory<'a> {
    holders: Cow<'a, [Address]>,
    burn: Option<&'a BurnFunding>,
}
impl<'a> Inventory<'a> {
    pub(super) fn for_order(o: &'a Order) -> Option<Self> {
        let (holders, burn): (Cow<'a, [Address]>, Option<&BurnFunding>) = match &o.wire {
            Wire::Vault(_) => (Cow::Borrowed(std::slice::from_ref(&o.target)), None),
            Wire::V5Funded(_, v) if !v.is_empty() => (Cow::Borrowed(v), None),
            Wire::V5Funded(_, _) => return None,
            Wire::V5Burn(_, b) => (
                Cow::Owned(b.credited.iter().map(|(a, _)| *a).collect()),
                Some(b),
            ),
            _ => (Cow::Borrowed(&[]), None),
        };
        if holders
            .iter()
            .enumerate()
            .any(|(i, a)| a.is_zero() || *a == o.payer || holders[..i].contains(a))
        {
            return None;
        }
        if burn.is_some_and(|b| {
            b.amount.is_zero()
                || !holders.contains(&b.burner)
                || b.credited
                    .iter()
                    .try_fold(b.amount, |sum, (_, credit)| sum.checked_add(*credit))
                    != Some(o.amount)
        }) {
            return None;
        }
        Some(Self { holders, burn })
    }
    pub(super) fn len(&self) -> usize {
        self.holders.len() + usize::from(self.burn.is_some())
    }
    pub(super) fn calls(&self, from: Address, to: Address) -> impl Iterator<Item = Call> + '_ {
        let make = move |input| Call { from, to, input };
        self.holders
            .iter()
            .map(move |holder| make(IMoney::balanceOfCall { owner: *holder }.abi_encode().into()))
            .chain(
                self.burn
                    .map(|_| make(IMoney::totalSupplyCall {}.abi_encode().into())),
            )
    }
    pub(super) fn balances(&self, frame: &[CallResult]) -> Option<(Vec<U256>, Vec<U256>)> {
        let n = self.len();
        if frame.len() < n * 2 {
            return None;
        }
        let amounts = |values: &[CallResult]| {
            values
                .iter()
                .map(|v| {
                    (v.ok() && v.return_data.len() == 32)
                        .then(|| U256::from_be_slice(&v.return_data))
                })
                .collect::<Option<Vec<_>>>()
        };
        Some((amounts(&frame[..n])?, amounts(&frame[frame.len() - n..])?))
    }
    pub(super) fn confirm(
        &self,
        order: &Order,
        before: &[U256],
        after: &[U256],
        logs: &[SimLog],
    ) -> bool {
        if self.len() != before.len() || self.len() != after.len() {
            return false;
        }
        if let Some(b) = self.burn {
            let n = self.holders.len();
            return before[n].checked_sub(after[n]) == Some(b.amount)
                && b.credited
                    .iter()
                    .enumerate()
                    .all(|(i, (_, expected))| after[i].checked_sub(before[i]) == Some(*expected))
                && b.logged(order.input, logs);
        }
        self.holders.is_empty() || net_funding(before, after) == Some(order.amount)
    }
}
impl BurnFunding {
    fn logged(&self, input: Address, logs: &[SimLog]) -> bool {
        let signature = alloy_primitives::keccak256("Transfer(address,address,uint256)");
        let topics = [
            signature,
            self.burner.into_word(),
            Address::ZERO.into_word(),
        ];
        let mut burned = U256::ZERO;
        for log in logs
            .iter()
            .filter(|l| l.address == input && l.topics.as_slice() == topics)
        {
            if log.data.len() != 32 {
                return false;
            }
            let Some(total) = burned.checked_add(U256::from_be_slice(&log.data)) else {
                return false;
            };
            burned = total;
        }
        burned == self.amount && !burned.is_zero()
    }
}

fn net_funding(before: &[U256], after: &[U256]) -> Option<U256> {
    if before.len() != after.len() {
        return None;
    }
    before
        .iter()
        .zip(after)
        .try_fold(U256::ZERO, |sum, (&old, &new)| {
            sum.checked_add(new.checked_sub(old)?)
        })
}

#[cfg(test)]
mod inventory_tests {
    use super::*;

    #[test]
    fn physical_funding_may_be_split_without_crediting_the_module() {
        let u = |value: u64| U256::from(value);
        assert_eq!(net_funding(&[u(7), u(13)], &[u(97), u(23)]), Some(u(100)));
        assert_eq!(net_funding(&[u(7), u(13)], &[u(97), u(22)]), Some(u(99)));
        assert_eq!(net_funding(&[u(7), u(13)], &[u(108), u(12)]), None);
        assert_eq!(
            net_funding(&[u(7), u(13)], &[u(7), u(13)]),
            Some(U256::ZERO)
        );
        let archive: serde_json::Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/vvs_ethereum_wallets.json.gz"
            )
            .as_slice(),
        ))
        .unwrap();
        let transfer = format!(
            "{:#x}",
            alloy_primitives::keccak256("Transfer(address,address,uint256)")
        );
        for row in archive["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["wire"] == "v5")
        {
            assert_eq!(row["before"][4], row["after"][4]);
            let pool = row["pool"]
                .as_str()
                .unwrap()
                .parse::<Address>()
                .unwrap()
                .into_word();
            let pool = format!("{pool:#x}");
            let mut total = U256::ZERO;
            for log in row["logs"].as_array().unwrap() {
                if log["topics"][0] == transfer
                    && log["topics"][2] == pool
                    && log["address"]
                        .as_str()
                        .unwrap()
                        .eq_ignore_ascii_case(row["source"].as_str().unwrap())
                {
                    total += U256::from_str_radix(
                        log["data"].as_str().unwrap().trim_start_matches("0x"),
                        16,
                    )
                    .unwrap();
                }
            }
            assert_eq!(
                total,
                row["spent"].as_str().unwrap().parse::<U256>().unwrap()
            );
        }
    }
    #[test]
    fn oracle_pair_burn_reconciles_with_exact_wallet_debit() {
        let archive: serde_json::Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/bundles_ethereum_wallets.json.gz"
            )
            .as_slice(),
        ))
        .unwrap();
        let input: Address = archive["protocol_token"].as_str().unwrap().parse().unwrap();
        let examples = archive["cases"].as_array().unwrap();
        let mut checked = 0;
        for row in examples
            .iter()
            .filter(|r| r["input"] == archive["protocol_token"])
        {
            let Some(raw) = row["logs"].as_array() else {
                continue;
            };
            let logs: Vec<SimLog> =
                serde_json::from_value(serde_json::Value::Array(raw.clone())).unwrap();
            let pair: Address = row["pool"].as_str().unwrap().parse().unwrap();
            let topics = [
                alloy_primitives::keccak256("Transfer(address,address,uint256)"),
                pair.into_word(),
                Address::ZERO.into_word(),
            ];
            let Some(event) = logs
                .iter()
                .find(|l| l.address == input && l.topics.as_slice() == topics)
            else {
                continue;
            };
            let burn = U256::from_be_slice(&event.data);
            let spent = row["amount"].as_str().unwrap().parse::<U256>().unwrap();
            assert!(burn > U256::ZERO && burn < spent);
            let mut proof = BurnFunding {
                credited: vec![(pair, spent - burn)],
                burner: pair,
                amount: burn,
            };
            assert!(proof.logged(input, &logs));
            proof.amount += U256::ONE;
            assert!(!proof.logged(input, &logs));
            checked += 1;
        }
        assert!(checked > 0);
    }
}
