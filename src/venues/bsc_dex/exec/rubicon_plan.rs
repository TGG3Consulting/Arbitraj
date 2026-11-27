//! Rebuild the exact maker orders at the accepted block before wallet signing.
use super::{
    carbon_binding::Code,
    chain::Step,
    plan::{DirectCall, Plan},
    rubicon_wallet::{IBookWallet, Proof, Terms},
};
use crate::core::types::rubicon::Book;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        pools::{dagrow, direct_module, registry::Entry, rubicon},
        quote::{v2_wallet_sim as sim, white_state},
        wallet::approve,
    },
};
use alloy_primitives::{Address, U256};
use serde_json::Value;

enum Size {
    ExactInput(U256),
    ExactOutput(U256),
    Budget { limit: U256, minimum: U256 },
}

impl Size {
    fn actions(&self, book: &Book, input: Address) -> Option<Vec<(U256, u128)>> {
        let fill = match *self {
            Self::ExactInput(amount) => book.plan(input, true, amount).filter(|p| p.complete)?,
            Self::ExactOutput(amount) => book.plan(input, false, amount).filter(|p| p.complete)?,
            Self::Budget { limit, minimum } => book
                .plan(input, true, limit)
                .filter(|p| !p.spent.is_zero() && p.spent <= limit && p.received >= minimum)?,
        };
        Some(fill.actions)
    }
}

pub(crate) fn terms(
    ctx: &Ctx,
    e: &Entry,
    book: &Book,
    input: Address,
    amount: U256,
    desired: Option<U256>,
) -> GatewayResult<Terms> {
    let size = desired.map_or(Size::ExactInput(amount), Size::ExactOutput);
    selected(ctx, e, book, input, size, desired)
}

fn selected(
    ctx: &Ctx,
    e: &Entry,
    book: &Book,
    input: Address,
    size: Size,
    desired: Option<U256>,
) -> GatewayResult<Terms> {
    let id = e.family.id;
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Rubicon Classic: объём или параметры кошелька не подтверждены",
        )
    };
    let target = ctx.dex_addr("dex.router", id)?;
    let hashes = direct_module::configured(ctx, id)?;
    let [wallet_hash] = hashes.as_slice() else {
        return Err(fail());
    };
    if target == book.market || target.is_zero() {
        return Err(fail());
    }
    let actions = size.actions(book, input).ok_or_else(fail)?;
    Ok(Terms {
        market: book.market,
        target,
        code: Code::profile(ctx, id)?,
        wallet_hash: *wallet_hash,
        actions,
        desired,
    })
}
pub(crate) async fn check(ctx: &Ctx, e: &Entry, t: &Terms, at: &Value) -> GatewayResult<()> {
    use crate::venues::bsc_dex::node::call;
    use alloy::sol_types::SolCall;
    let id = e.family.id;
    let out = |what| ctx.err(ErrorCode::BadResponse, what);
    let ((), market, code) = tokio::try_join!(
        direct_module::verify_code(ctx, id, t.target, at),
        crate::venues::bsc_dex::node::rpc::call::<alloy_primitives::Bytes>(
            ctx,
            "eth_call",
            serde_json::json!([{"to":t.target,"data":alloy_primitives::Bytes::from(IBookWallet::marketCall{}.abi_encode())},at])
        ),
        crate::venues::bsc_dex::node::rpc::call::<alloy_primitives::Bytes>(
            ctx,
            "eth_call",
            serde_json::json!([{"to":t.target,"data":alloy_primitives::Bytes::from(IBookWallet::marketCodeHashCall{}.abi_encode())},at])
        )
    )?;
    if call::decode::<IBookWallet::marketCall>(&market)? != t.market
        || call::decode::<IBookWallet::marketCodeHashCall>(&code)? != t.code.shell
    {
        return Err(out("Rubicon Classic: модуль привязан к другому рынку"));
    }
    Ok(())
}
pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    payer: Address,
    height: u64,
    selected: Option<(Address, Option<U256>)>,
) -> GatewayResult<Plan> {
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Rubicon Classic: изменились заявки или условия покупки",
        )
    };
    let (target, desired) = selected.ok_or_else(fail)?;
    if !s.from_wallet || !s.to_wallet {
        return Err(fail());
    }
    let g = ctx.settings.generation();
    let e = &s.entry;
    let o = &s.order;
    let h = white_state::head(ctx, &format!("{height:#x}")).await?;
    let at = h.selector();
    let book = rubicon::load(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        &at,
    )
    .await?;
    let t = terms(ctx, e, &book, o.token_in, o.amount_in, desired)?;
    if target != t.target {
        return Err(fail());
    }
    check(ctx, e, &t, &at).await?;
    let order = sim::Order {
        payer,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: sim::Wire::Rubicon(Box::new(t.clone())),
    };
    let deadline = U256::from(
        h.timestamp
            .to::<u64>()
            .checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(fail)?,
    );
    let proof: Proof = t.proof(&order, o.min_out, deadline);
    sim::measure(ctx, h, order, true).await?;
    let allowed = dagrow::allowance(ctx, o.token_in, payer, target, &at).await?;
    let approvals = approve::fork_exact(ctx, o.token_in, target, Some(allowed), o.amount_in)?;
    if ctx.settings.generation() != g {
        return Err(fail());
    }
    let mut built = Plan::new(
        ctx,
        (approvals, target, payer),
        o.token_out,
        vec![],
        o.min_out,
        deadline,
    )?;
    built.set_direct_market(DirectCall::Rubicon(std::sync::Arc::new(proof)));
    Ok(built)
}

pub(crate) fn probe(
    ctx: &Ctx,
    e: &Entry,
    book: &Book,
    o: &super::plan::Order,
    payer: Address,
    now: u64,
) -> GatewayResult<(Vec<approve::Approval>, Address, alloy_primitives::Bytes)> {
    let t = selected(
        ctx,
        e,
        book,
        o.token_in,
        Size::Budget {
            limit: o.amount_in,
            minimum: o.min_out,
        },
        None,
    )?;
    let target = t.target;
    let p = Proof {
        terms: t,
        payer,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        deadline: U256::from(now.checked_add(ctx.int("dex.deadline_s")?).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "Rubicon Classic: неверный срок поручения",
            )
        })?),
    };
    Ok((
        approve::fork_exact(ctx, o.token_in, target, None, o.amount_in)?,
        target,
        p.data(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::rubicon::Offer;

    #[test]
    fn full_archive_book_refunds_one_raw_unit_under_budget_only() {
        let snapshot: serde_json::Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            &include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/rubicon_classic_base.json.gz"
            )[..],
        ))
        .unwrap();
        let rows = snapshot["controls"].as_array().unwrap();
        for row in rows {
            let address = |key: &str| row[key].as_str().unwrap().parse::<Address>().unwrap();
            let units = |key: &str| row[key].as_str().unwrap().parse::<U256>().unwrap();
            let source = address("source");
            let target = address("target");
            let offers = snapshot["active"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|o| o["source"] == row["source"] && o["target"] == row["target"])
                .map(|o| Offer {
                    id: U256::from(o["id"].as_u64().unwrap()),
                    available: o["quantity"].as_str().unwrap().parse().unwrap(),
                    requested: o["price"].as_str().unwrap().parse().unwrap(),
                })
                .collect::<Vec<_>>();
            let mut offers = offers;
            offers.sort_by(|a, b| {
                (U256::from(a.requested) * U256::from(b.available))
                    .cmp(&(U256::from(b.requested) * U256::from(a.available)))
            });
            let total: U256 = offers.iter().map(|o| U256::from(o.available)).sum();
            let book = Book {
                tokens: [source, target],
                market: Address::ZERO,
                dust: [U256::ZERO; 2],
                offers: [offers, vec![]],
                fees: snapshot["fees"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u32)
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap(),
            };
            let limit = units("budget");
            let minimum = units("received");
            assert_eq!(limit - units("spent"), U256::ONE);
            assert!(
                Size::Budget { limit, minimum }
                    .actions(&book, source)
                    .is_some()
            );
            assert!(
                Size::Budget {
                    limit,
                    minimum: total + U256::ONE
                }
                .actions(&book, source)
                .is_none()
            );
            if total == minimum {
                assert!(Size::ExactInput(limit).actions(&book, source).is_none());
                assert!(
                    Size::ExactInput(units("spent"))
                        .actions(&book, source)
                        .is_some()
                );
                assert!(Size::ExactOutput(minimum).actions(&book, source).is_some());
            }
        }
    }
}
