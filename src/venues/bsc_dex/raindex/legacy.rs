//! У V3 и ранней книги V4 Base отсутствует внешний quote. Измеряем настоящие разрешённые размеры своего кошелька.
//! Оценка между точками проверяется полной симуляцией конкретной выплаты перед созданием билета.
use super::*;
use crate::{
    settings::scope::Scope,
    venues::bsc_dex::{math::full_math::mul_div_up, wallet},
};
use alloy::sol_types::SolCall;
use serde_json::json;

/// У раннего Base V4 есть takeOrders2 и orderExists, но в его конкретном runtime нет quote.
/// Обычный отказ котировки других книг не переключает их на этот режим.
pub(super) fn output_samples(p: Profile) -> bool {
    p.version == 3 || early_base_v4(p)
}

fn early_base_v4(p: Profile) -> bool {
    p.chain == 8453 && p.version == 4 &&
        p.book == alloy_primitives::address!("a2f56f8f74b7d04d61f281be6576b6155581dcba") &&
        p.book_hash == alloy_primitives::b256!("457d9d27a8653616f06cf389b80b4dbce756cd37cb06f50391e119d0fbe21557")
}

pub(super) fn budget_allowed(p: Profile, maximum: Option<U256>, wanted: U256) -> bool {
    !early_base_v4(p) || maximum.is_some_and(|cap| wanted <= cap)
}

#[derive(Clone, Copy)]
struct Sample {
    input: Address,
    output: Address,
    maximum: U256,
    target: U256,
}
impl Sample {
    fn configured(ctx: &Ctx, leg: &Leg) -> Vec<Self> {
        let Some(v) = ctx.settings.get_exact(
            "raindex.sample_amounts",
            &Scope::Venue(ctx.place.id.to_string()),
        ) else {
            return vec![];
        };
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| {
                let fields = row.as_str()?.split_whitespace().collect::<Vec<_>>();
                let [chain, book, input, output, cap, qty] = fields.as_slice() else {
                    return None;
                };
                if chain.parse::<u64>().ok()? != leg.p.chain
                    || book.parse::<Address>().ok()? != leg.p.book
                {
                    return None;
                }
                let s = Self {
                    input: input.parse().ok()?,
                    output: output.parse().ok()?,
                    maximum: cap.parse().ok()?,
                    target: qty.parse().ok()?,
                };
                (s.input == leg.input
                    && s.output == leg.output
                    && !s.maximum.is_zero()
                    && !s.target.is_zero())
                .then_some(s)
            })
            .collect()
    }
}
fn price(debit: U256, credit: U256, di: u8, dout: u8) -> Option<Quantity> {
    let exp = 18 + i32::from(dout) - i32::from(di);
    let pow = U256::from(10).checked_pow(U256::from(exp.unsigned_abs()))?;
    let coefficient = if exp >= 0 {
        mul_div_up(debit, pow, credit)?
    } else {
        debit.div_ceil(credit.checked_mul(pow)?)
    };
    Quantity::v4(coefficient)
}
pub(super) fn registered(response: &[u8]) -> bool {
    matches!(wire::orderExistsCall::abi_decode_returns_validate(response), Ok(true))
}

pub(super) async fn read(p: &Parts, leg: Leg, h: white_state::Head) -> Vec<Live> {
    let samples = Sample::configured(&p.ctx, &leg);
    if samples.is_empty() || leg.via.is_some() {
        return vec![];
    }
    let Some(bytes)=rpc::call::<Bytes>(&p.ctx,"eth_call",json!([{"to":leg.p.book,"input":Bytes::from(wire::orderExistsCall{hash:leg.hash}.abi_encode())},h.selector()])).await.ok() else{return vec![];};
    if !registered(&bytes) {
        return vec![];
    }
    let Ok(owner) = wallet::signer::address(&p.ctx) else {
        return vec![];
    };
    if owner == leg.order.owner() {
        return vec![];
    }
    let (Ok(di), Ok(dout)) = tokio::join!(
        p.tokens.decimals(&p.ctx, leg.input),
        p.tokens.decimals(&p.ctx, leg.output)
    ) else {
        return vec![];
    };
    let Some(expiry) = p
        .ctx
        .int("dex.deadline_s")
        .ok()
        .and_then(|d| h.timestamp.to::<u64>().checked_add(d))
    else {
        return vec![];
    };
    let mut out = vec![];
    for s in samples {
        let Some(action) =
            leg.order
                .take(leg.i, leg.j, s.target, dout, true, B256::repeat_byte(255))
        else {
            continue;
        };
        let pay = payment::Payment::route(
            &leg,
            owner,
            action,
            expiry,
            (U256::ONE, s.maximum, s.target),
            U256::ZERO,
        );
        if let Ok((spent, credit, _)) = pay.measure(&p.ctx, h, expiry).await
            && credit == s.target
            && let Some(ratio) = price(spent, credit, di, dout)
            && let Some(cap) = Quantity::float(Quantity::units(credit, dout).unwrap_or_default())
        {
            out.push(Live {
                leg: leg.clone(),
                cap,
                ratio,
                max_budget: Some(s.maximum),
            });
        }
    }
    out
}
