//! Подтверждение прямой продажи или покупки Ammalgam по одной подписанной транзакции.
//! Контракт измеряет поступившие в пару средства и остаток получателя. EOA и исходный пул
//! должны испустить согласованные события, прежде чем восстановление признает исполнение.
use super::{empire_wallet::Proof, fill};
use crate::venues::bsc_dex::pools::abi_v2v3::IV2Pair;
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, U256};

alloy::sol! {event Filled(address indexed pair,address indexed payer,address indexed recipient,address input,address output,
uint256 spent,uint256 funded,uint256 received);}
fn unique<T: SolEvent>(logs: &[Log], emitter: Address) -> Option<T> {
    let mut hits = logs
        .iter()
        .filter(|l| l.address() == emitter && l.topic0() == Some(&T::SIGNATURE_HASH));
    let value = T::decode_log_data(&hits.next()?.inner.data).ok()?;
    hits.next().is_none().then_some(value)
}
pub(super) fn amounts(
    p: &Proof,
    logs: &[Log],
    payer: Address,
    input: Address,
    output: Address,
) -> Option<(U256, U256)> {
    if (p.payer, p.input, p.output) != (payer, input, output) || input == output {
        return None;
    }
    let f = unique::<Filled>(logs, p.module)?;
    let s = unique::<IV2Pair::Swap>(logs, p.pool)?;
    let (paid_to_pool, paid_out, other_out) = if input < output {
        (s.amount0In, s.amount1Out, s.amount0Out)
    } else {
        (s.amount1In, s.amount0Out, s.amount1Out)
    };
    if (f.pair, f.payer, f.recipient, f.input, f.output) != (p.pool, payer, payer, input, output)
        || f.spent != p.amount
        || f.funded.is_zero()
        || f.funded > f.spent
        || f.received.is_zero()
        || f.received < p.minimum
        || s.sender != p.module
        || s.to != payer
        || paid_to_pool < f.funded
        || paid_out.is_zero()
        || !other_out.is_zero()
    {
        return None;
    }
    let transfers = |coin, from, to| fill::moved_logs(logs, coin, from, to);
    let debit =
        transfers(input, Some(payer), None).checked_sub(transfers(input, None, Some(payer)))?;
    let credit = transfers(output, Some(p.pool), Some(payer));
    (debit == f.spent
        && !credit.is_zero()
        && transfers(output, None, Some(payer)) == credit
        && transfers(output, Some(payer), None).is_zero())
    .then_some((f.spent, f.received))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn ten_signed_market_payments_recover_once_with_the_original_participants() {
        let raw = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ammalgam_ethereum_wallet.json.gz"
        );
        let book: Value =
            serde_json::from_reader(flate2::read::GzDecoder::new(raw.as_slice())).unwrap();
        let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let number = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let mut count = 0;
        for row in book["controls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["mode"] == "wallet")
        {
            let mut logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
            let module = address(&book["module"]["address"]);
            let observed = unique::<Filled>(&logs, module).unwrap();
            let p = Proof {
                chain: crate::settings::evm_profiles::ETHEREUM_MAINNET,
                module,
                pool: address(&row["pool"]),
                payer: observed.payer,
                input: address(&row["input"]),
                output: address(&row["output"]),
                amount: number(&row["amount"]),
                minimum: number(&row["received"]),
                deadline: U256::ONE,
            };
            assert_eq!(
                amounts(&p, &logs, p.payer, p.input, p.output),
                Some((p.amount, p.minimum))
            );
            assert!(amounts(&p, &logs, p.payer, p.output, p.input).is_none());
            let mut wrong = p.clone();
            wrong.minimum += U256::ONE;
            assert!(amounts(&wrong, &logs, p.payer, p.input, p.output).is_none());
            let duplicate = logs
                .iter()
                .find(|l| {
                    l.address() == p.pool && l.topic0() == Some(&IV2Pair::Swap::SIGNATURE_HASH)
                })
                .unwrap()
                .clone();
            logs.push(duplicate);
            assert!(amounts(&p, &logs, p.payer, p.input, p.output).is_none());
            count += 1;
        }
        assert_eq!(count, 10);
    }
}
