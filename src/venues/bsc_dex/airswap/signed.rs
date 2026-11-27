//! Проверка разрешений, подписи маркетмейкера и фактических сумм симуляции.
use super::{
    Limits,
    client::{Maker, Query},
    profile, receipt, routing, timely, wire,
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx, exec::signed_call::Erc20Call, node::call, pools::exact_wallet_tokens,
        quote::white_state,
    },
};
use alloy_primitives::{Address, U256, keccak256};
pub(super) async fn prepare(
    c: &Ctx,
    q: Query,
    scope: profile::Profile,
    maker: &Maker,
    who: Address,
    head: white_state::Head,
    required: Option<U256>,
) -> GatewayResult<Option<(alloy_primitives::B256, U256)>> {
    if scope.module.is_none() {
        exact_wallet_tokens::check(c, "airswap", q.input, q.output, &head.selector()).await?;
    }
    let terms = scope.check(c, head, who).await?;
    if let Some(spend) = required {
        use crate::venues::bsc_dex::pools::dagrow;
        let at = head.selector();
        let (balance, grant) = tokio::try_join!(
            dagrow::balance(c, q.input, who, &at),
            dagrow::allowance(c, q.input, who, scope.target(), &at)
        )?;
        if spend.is_zero() || spend > balance || spend > grant {
            return Ok(None);
        }
    }
    if !maker.supports(c, q, scope).await? {
        return Ok(None);
    }
    Ok(Some(terms))
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn signed(
    c: &Ctx,
    q: Query,
    limits: Limits,
    head: white_state::Head,
    (di, dout): (u8, u8),
    scope: profile::Profile,
    maker: &Maker,
    (generation, who, ms): (u64, Address, u64),
    (domain, fee): (alloy_primitives::B256, U256),
) -> GatewayResult<Vec<routing::Ticket>> {
    let mut out = Vec::new();
    let sender = scope.sender(who);
    let order = maker.order(c, q, scope, sender, ms).await?;
    if !timely(c, order.expiry)
        || order.amount > limits.maximum
        || order.fee != fee
        || order.maker == who
        || order.maker == sender
        || order.maker == scope.exchange.address
    {
        return Ok(out);
    }
    let at = head.selector();
    let Ok(authorized) = call::canonical(
        c,
        &at,
        scope.exchange.address,
        wire::ISwap::authorizedCall {
            signerWallet: order.maker,
        },
    )
    .await
    else {
        return Ok(out);
    };
    let signatory = if authorized.is_zero() {
        order.maker
    } else {
        authorized
    };
    if !matches!(
        call::canonical(
            c,
            &at,
            scope.exchange.address,
            wire::ISwap::nonceUsedCall {
                signatory,
                nonce: order.nonce
            }
        )
        .await,
        Ok(false)
    ) {
        return Ok(out);
    }
    for any in [false, true] {
        let signed_for = if any { Address::ZERO } else { sender };
        if order.sender.is_some_and(|s| s != signed_for)
            || (scope.module.is_none() && order.quoted < limits.minimum)
        {
            continue;
        }
        let minimum = if scope.module.is_none() {
            order.quoted
        } else {
            limits.minimum
        };
        let mut data = order.data(who, scope.module.is_some(), any, minimum);
        let mut proof = receipt::Saved {
            call: Erc20Call {
                chain: scope.chain,
                payer: who,
                module: scope.target(),
                calldata: keccak256(&data),
            },
            profile: scope,
            maker: order.maker,
            nonce: order.nonce,
            digest: order.digest(domain, signed_for),
            expiry: order.expiry,
            input: q.input,
            output: q.output,
            amount: order.amount,
            quoted: order.quoted,
            fee,
            minimum,
        };
        if let Ok((spent, received, gas)) = proof.measure(c, head, &data).await
            && spent <= limits.maximum
            && received >= limits.minimum
            && timely(c, proof.expiry)
        {
            let gas = if scope.module.is_some() && received > minimum {
                proof.minimum = received;
                data = order.data(who, true, any, received);
                proof.call.calldata = keccak256(&data);
                let Ok((paid, credit, g)) = proof.measure(c, head, &data).await else {
                    continue;
                };
                if (paid, credit) != (spent, received) {
                    continue;
                }
                g
            } else {
                gas
            };
            out.push(routing::Ticket {
                signatory,
                proof,
                data,
                spent,
                received,
                gas,
                generation,
                di,
                dout,
            });
        }
    }

    Ok(out)
}
