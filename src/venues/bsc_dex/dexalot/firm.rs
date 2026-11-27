//! Проверяет бюджет и допуски кошелька, сравнивает варианты firm_option для ручного и автоматического запроса.
use super::{
    Query, enabled, fail,
    firm_option::{Attempt, option},
    profile::Config,
};
use crate::{
    gateway::{
        errors::GatewayResult,
        source_quote::{FirmTerms, Preview},
    },
    venues::bsc_dex::{
        num,
        parts::Parts,
        pools::{dagrow, exact_wallet_tokens},
        quote::white_state,
        wallet,
    },
};
use alloy_primitives::{Address, U256};
use serde_json::json;
use std::sync::Arc;

pub(super) async fn report(
    p: &Parts,
    q: Query,
    f: FirmTerms,
    trader: Option<Address>,
    decs: (u8, u8),
    expected: Option<U256>,
) -> GatewayResult<Preview> {
    let account = p.bound(&f.wallet);
    let ctx = &account.ctx;
    ctx.bound()?;
    let who = wallet::signer::address(ctx)?;
    let bad = || {
        fail(
            ctx,
            "Dexalot: проверьте разрешение, сумму, порог, сроки и собственный кошелёк",
        )
    };
    if !enabled(ctx) || trader != Some(who) || f.fee_tenths_bps.is_some() {
        return Err(bad());
    }
    let maximum = match (q.by_target, f.maximum_raw.as_deref()) {
        (false, None) => q.qty,
        (true, Some(x)) => num::exact_amount(Some(x), None, 0).ok_or_else(bad)?,
        _ => return Err(bad()),
    };
    let minimum = num::exact_amount(Some(&f.minimum_raw), None, 0)
        .ok_or_else(bad)?
        .max(if q.by_target { q.qty } else { U256::ONE });
    let chain = ctx.chain_id().ok_or_else(bad)?;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let map = |a: Address| if a.is_zero() { wrapped } else { a };
    let (input, output) = (map(q.input), map(q.output));
    if input == output {
        return Err(bad());
    }
    let (di, dout) = tokio::try_join!(
        account.tokens.decimals(ctx, input),
        account.tokens.decimals(ctx, output)
    )?;
    if (di, dout) != decs {
        return Err(bad());
    }
    let gen_before = ctx.settings.generation();
    let config = Config::read(ctx)?;
    let h = white_state::head(ctx, "latest").await?;
    let bal = dagrow::balance(ctx, input, who, &h.selector()).await?;
    let required = expected.unwrap_or(if q.by_target { U256::ONE } else { q.qty });
    if bal < required || required.is_zero() || required > maximum {
        return Err(bad());
    }
    let mut choices = Vec::new();
    let mapped_direct = expected.is_some();
    for m in &config.modules {
        if config.ready(ctx, who, Some(*m), h).await.is_ok()
            && dagrow::allowance(ctx, input, who, m.addr, &h.selector())
                .await
                .is_ok_and(|v| v >= required)
        {
            choices.push(Some(*m));
        }
    }
    // A bare immutable maker can be considered for an exact ERC20 pair, a zero slippage flag and the owner's own payee.
    if (mapped_direct || (!q.input.is_zero() && !q.output.is_zero()))
        && exact_wallet_tokens::check(ctx, "dexalot", input, output, &h.selector())
            .await
            .is_ok()
    {
        for m in &config.makers {
            if m.proxy.is_none()
                && dagrow::allowance(ctx, input, who, m.addr, &h.selector())
                    .await
                    .is_ok_and(|v| v >= required)
            {
                choices.push(None);
                break;
            }
        }
    }
    if choices.is_empty() {
        return Err(fail(
            ctx,
            "Dexalot: нет настроенного модуля или неизменяемого maker с разрешением от владельца",
        ));
    }
    if expected.is_some() {
        let mut selected = None;
        for module in choices {
            if config.ready(ctx, who, module, h).await.is_ok()
                && config.check_makers(ctx, module.is_some(), h).await.is_ok()
            {
                selected = Some(module);
                break;
            }
        }
        choices = selected.into_iter().collect();
    }
    let mut tickets = Vec::new();
    let mut last = None;
    for m in choices {
        if ctx.settings.generation() != gen_before {
            break;
        }
        let scope = Attempt {
            chain,
            who,
            input,
            output,
            minimum,
            maximum,
            module: m,
            di,
            dout,
            generation: gen_before,
        };
        let order = if mapped_direct && m.is_none() {
            Query { input, output, ..q }
        } else {
            q
        };
        match option(&account, &config, order, h, scope).await {
            Ok(t) => tickets.push(Arc::new(t)),
            Err(e) => last = Some(e),
        }
    }
    if ctx.settings.generation() != gen_before {
        return Err(bad());
    }
    let best = tickets
        .iter()
        .min_by(|a, b| {
            if q.by_target {
                a.spent.cmp(&b.spent).then(a.gas.cmp(&b.gas))
            } else {
                b.received.cmp(&a.received).then(a.gas.cmp(&b.gas))
            }
        })
        .cloned()
        .ok_or_else(|| last.unwrap_or_else(bad))?;
    let report = Preview {
        source: "dexalot".into(),
        input,
        output,
        spent_raw: best.spent.to_string(),
        received_raw: best.received.to_string(),
        unspent_raw: maximum.checked_sub(best.spent).ok_or_else(bad)?.to_string(),
        fee: None,
        block_hash: format!("{:#x}", best.head.hash),
        block: Some(best.head.number.to()),
        expires: Some(best.proof.expiry),
        details: json!({"type":"wallet_simulation","physical_source":q.input,"physical_destination":q.output,"maker":best.proof.profile.maker.addr,"nonce":best.proof.nonce,"signed_digest":best.proof.digest,"signed_out":best.proof.quoted.to_string(),"minimum":best.proof.minimum.to_string(),"gas":best.gas,"candidates":tickets.len(),"allow_trade":f.allow_trade}),
    };
    if f.allow_trade {
        for t in tickets {
            p.dexalot.put(t)
        }
    }
    Ok(report)
}
