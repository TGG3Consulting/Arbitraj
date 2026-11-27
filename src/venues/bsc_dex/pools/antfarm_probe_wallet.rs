//! Pick a measurable Antfarm probe leg after replaying its real prefix on a single canonical block.
use super::super::{
    check::simulate::{self, Base, Call, CallResult},
    ctx::Ctx,
    exec::{antfarm_output_wallet as output, antfarm_wallet as input, plan::Order},
    math::{antfarm_output::Funds, full_math::mul_div},
    node::rpc,
    quote::{gas_base, white_state::Head},
    wallet::approve::{self, Approval},
};
use super::{antfarm, antfarm_book, registry::Entry};
use crate::{
    core::types::{AntfarmBook, AntfarmMarket, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256, address};
use serde_json::{Value, json};

sol! {interface IERC {
    function balanceOf(address owner) external view returns(uint256);
    function allowance(address owner,address spender) external view returns(uint256);
}}
pub(in crate::venues::bsc_dex) const VAULT: Address =
    address!("ba12222222228d8ba445958a75a0704d566bf2c8");
type Parts = (Vec<Approval>, Address, Bytes);
type OrderData = (Address, Bytes, Option<(Address, U256)>);
pub(in crate::venues::bsc_dex) struct Choice {
    pub entry: Box<Entry>,
    pub call: Parts,
}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Antfarm: контрольный обмен не подтвердил источник комиссии и выплату",
    )
}
fn book(e: &Entry) -> Option<&AntfarmBook> {
    match &e.pool.state {
        PoolState::Antfarm(b) => Some(b),
        _ => None,
    }
}
fn call<C: SolCall>(payer: Address, to: Address, c: C) -> Call {
    Call {
        from: payer,
        to,
        input: c.abi_encode().into(),
    }
}
fn units(r: &CallResult) -> Option<U256> {
    (r.ok() && r.return_data.len() == 32).then(|| U256::from_be_slice(&r.return_data))
}
fn order_data(
    ctx: &Ctx,
    e: &Entry,
    o: &Order,
    now: u64,
) -> GatewayResult<OrderData> {
    let b = book(e).ok_or_else(|| bad(ctx))?;
    let deadline = U256::from(
        now.checked_add(ctx.int("dex.deadline_s")?)
            .ok_or_else(|| bad(ctx))?,
    );
    if let Some((tier, funds)) = b.proceeds {
        let target = ctx.dex_addr("dex.antfarm_output_wallet", e.family.id)?;
        let v = b
            .output_order(o.token_in, true, o.amount_in)
            .ok_or_else(|| bad(ctx))?;
        let terms = output::Terms::new(
            target,
            o.token_in,
            o.token_out,
            b.market.tier,
            tier,
            matches!(funds, Funds::Vault { .. }),
            &v,
        );
        Ok((
            target,
            terms.data(o.min_out, deadline),
            terms.stock_approval(),
        ))
    } else {
        let target = ctx.dex_addr("dex.router", e.family.id)?;
        let terms = input::Terms::from_budget(b, target, o.token_in, o.token_out, o.amount_in)
            .ok_or_else(|| bad(ctx))?;
        Ok((target, terms.data(o.min_out, deadline), None))
    }
}
pub(in crate::venues::bsc_dex) fn part(
    ctx: &Ctx,
    e: &Entry,
    o: &Order,
    now: u64,
) -> GatewayResult<Parts> {
    if book(e).is_none_or(|b| b.proceeds.is_none()) {
        return input::probe(ctx, e, o, now);
    }
    let (target, data, stock) = order_data(ctx, e, o, now)?;
    let mut a = approve::fork_all(ctx, o.token_in, target, o.amount_in)?;
    if let Some((t, n)) = stock {
        a.extend(approve::fork_all(ctx, t, target, n)?);
    }
    Ok((a, target, data))
}
struct Frame<'a> {
    ctx: &'a Ctx,
    h: Head,
    wallet: Address,
    prefix: &'a [Call],
}
impl Frame<'_> {
    async fn run(&self, tail: &[Call]) -> GatewayResult<Vec<CallResult>> {
        let mut calls = self.prefix.to_vec();
        calls.extend_from_slice(tail);
        let mut all = simulate::run_anchored(
            self.ctx,
            &calls,
            &[],
            &[],
            Base::timed(self.h.number.to(), self.h.timestamp.to()),
            Some(self.h.hash),
        )
        .await?;
        if all.len() != calls.len() || all[..self.prefix.len()].iter().any(|r| !r.ok()) {
            return Err(bad(self.ctx));
        }
        Ok(all.split_off(self.prefix.len()))
    }
    async fn trial(
        &self,
        e: &Entry,
        o: &Order,
        allowance: [U256; 2],
    ) -> GatewayResult<(U256, u64, Parts)> {
        let (target, data, stock) = order_data(self.ctx, e, o, self.h.timestamp.to())?;
        let mut aps = approve::fork_known(self.ctx, o.token_in, target, allowance[0], o.amount_in)?;
        if let Some((asset, n)) = stock {
            aps.extend(approve::fork_known(
                self.ctx,
                asset,
                target,
                allowance[1],
                n,
            )?);
        }
        let mut calls: Vec<_> = aps
            .iter()
            .map(|(to, d)| Call {
                from: self.wallet,
                to: *to,
                input: d.clone(),
            })
            .collect();
        let bal = |asset| {
            call(
                self.wallet,
                asset,
                IERC::balanceOfCall { owner: self.wallet },
            )
        };
        calls.extend([
            bal(o.token_in),
            bal(o.token_out),
            Call {
                from: self.wallet,
                to: target,
                input: data.clone(),
            },
            bal(o.token_in),
            bal(o.token_out),
        ]);
        let r = self.run(&calls).await?;
        if r.iter().any(|v| !v.ok()) {
            return Err(bad(self.ctx));
        }
        let j = aps.len();
        let spent = units(&r[j]).and_then(|n| n.checked_sub(units(&r[j + 3])?));
        let net = units(&r[j + 4])
            .and_then(|n| n.checked_sub(units(&r[j + 1])?))
            .filter(|v| *v >= o.min_out)
            .ok_or_else(|| bad(self.ctx))?;
        if spent != Some(o.amount_in) {
            return Err(bad(self.ctx));
        }
        let gas = r[..j]
            .iter()
            .chain(std::iter::once(&r[j + 2]))
            .map(|v| v.gas_used.to::<u64>())
            .sum();
        Ok((net, gas, (aps, target, data)))
    }
}
// Spot conversion for the diagnostic observation, expressed in its own output token. Ordinary trading has its own gas pricing.
struct GasMark {
    price: U256,
    wrapped: Address,
    reference: Option<AntfarmMarket>,
}
impl GasMark {
    async fn load(ctx: &Ctx, e: &Entry, h: Head) -> Option<Self> {
        let wrapped = ctx.addr("dex.native_wrapped").ok()?;
        let header: Value = rpc::call(ctx, "eth_getBlockByHash", json!([h.hash, false]))
            .await
            .ok()?;
        let base = serde_json::from_value::<U256>(header["baseFeePerGas"].clone()).ok()?;
        let price = gas_base::planned(ctx, base).ok()?;
        let reference = if !book(e)?.market.tokens.contains(&wrapped) {
            let tokens = [antfarm::ATF, wrapped];
            antfarm::market(
                ctx,
                e.family,
                antfarm::derived(tokens, 10),
                (tokens[0], tokens[1]),
                &h.selector(),
            )
            .await
            .ok()
        } else {
            None
        };
        Some(Self {
            price,
            wrapped,
            reference,
        })
    }
    fn value(&self, b: &AntfarmBook, token: Address, gas: u64) -> Option<U256> {
        fn convert(m: &AntfarmMarket, from: Address, x: U256) -> Option<U256> {
            let i = m.tokens.iter().position(|a| *a == from)?;
            mul_div(x, m.reserves[1 - i], m.reserves[i])
        }
        let amount = self.price.checked_mul(U256::from(gas))?;
        if self.wrapped == token {
            return Some(amount);
        }
        if b.market.tokens.contains(&self.wrapped) {
            return convert(&b.market, self.wrapped, amount);
        }
        let atf = convert(self.reference.as_ref()?, self.wrapped, amount)?;
        if token == antfarm::ATF {
            return Some(atf);
        }
        let (fund, _) = b
            .funding
            .iter()
            .filter(|(m, _)| m.tokens[1] == token)
            .max_by_key(|(m, _)| m.reserves[0])?;
        convert(fund, antfarm::ATF, atf)
    }
}
struct Score {
    net: U256,
    gas: u64,
    mark: Option<U256>,
}
fn better(a: &Score, b: &Score) -> bool {
    match (a.mark, b.mark) {
        (Some(x), Some(y)) => {
            (a.net.saturating_add(y), std::cmp::Reverse(a.gas))
                > (b.net.saturating_add(x), std::cmp::Reverse(b.gas))
        }
        _ => (a.net, std::cmp::Reverse(a.gas)) > (b.net, std::cmp::Reverse(b.gas)),
    }
}
pub(in crate::venues::bsc_dex) async fn select(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
    wallet: Address,
    o: &Order,
    prefix: &[Call],
) -> GatewayResult<Choice> {
    let frame = Frame {
        ctx,
        h,
        wallet,
        prefix,
    };
    let source = book(e).ok_or_else(|| bad(ctx))?;
    let tag = h.selector();
    let (normal, module) = tokio::join!(
        antfarm_book::module(ctx, e.family, &tag),
        output::module(ctx, e, h)
    );
    // With only the established input module preserve its original diagnostic and exact revert.
    if module.is_err() {
        normal?;
        let mut entry = Box::new(e.clone());
        if let PoolState::Antfarm(b) = &mut entry.pool.state {
            b.proceeds = None;
        }
        let call = part(ctx, &entry, o, h.timestamp.to())?;
        return Ok(Choice { entry, call });
    }
    let target = module?;
    let ordinary = normal.ok();
    let allowance = |token, spender| {
        call(
            wallet,
            token,
            IERC::allowanceCall {
                owner: wallet,
                spender,
            },
        )
    };
    let queries = [
        call(wallet, antfarm::ATF, IERC::balanceOfCall { owner: wallet }),
        call(wallet, o.token_out, IERC::balanceOfCall { owner: VAULT }),
        allowance(o.token_in, ordinary.unwrap_or(target)),
        allowance(o.token_in, target),
        allowance(antfarm::ATF, target),
    ];
    let obs = frame.run(&queries).await?;
    let mut modes = Vec::new();
    if let Some(a) = units(&obs[2]).filter(|_| ordinary.is_some()) {
        modes.push((None, [a, U256::ZERO]));
    }
    let stock = units(&obs[0])
        .zip(units(&obs[4]))
        .filter(|(n, _)| !n.is_zero());
    let loan = if let Some(balance) = units(&obs[1]).filter(|v| !v.is_zero()) {
        match output::funds(ctx, wallet, o.token_out, h, true).await {
            Ok(Funds::Vault { fee_per_1e18, .. }) => Some(Funds::Vault {
                output_balance: balance,
                fee_per_1e18,
            }),
            _ => None,
        }
    } else {
        None
    };
    if let Some(a) = units(&obs[3]) {
        for (f, _) in &source.funding {
            if f.tokens != [antfarm::ATF, o.token_out] {
                continue;
            }
            if let Some((n, allowed)) = stock {
                modes.push((Some((f.tier, Funds::Wallet { atf: n })), [a, allowed]));
            }
            if let Some(funds) = loan {
                modes.push((Some((f.tier, funds)), [a, U256::ZERO]));
            }
        }
    }
    let mark = if modes.len() > 1 {
        GasMark::load(ctx, e, h).await
    } else {
        None
    };
    let mut best: Option<(Score, Choice)> = None;
    for (mode, allowed) in modes {
        let mut entry = Box::new(e.clone());
        let PoolState::Antfarm(b) = &mut entry.pool.state else {
            return Err(bad(ctx));
        };
        b.proceeds = mode;
        let Ok((net, gas, call)) = frame.trial(&entry, o, allowed).await else {
            continue;
        };
        let score = Score {
            net,
            gas,
            mark: mark
                .as_ref()
                .and_then(|m| m.value(source, o.token_out, gas)),
        };
        if best.as_ref().is_none_or(|(old, _)| better(&score, old)) {
            best = Some((score, Choice { entry, call }));
        }
    }
    if let Some((_, choice)) = best {
        return Ok(choice);
    }
    // Preserve the established measurement of a genuine transfer/sale failure when the optional candidates failed.
    if ordinary.is_some() {
        let mut entry = Box::new(e.clone());
        if let PoolState::Antfarm(b) = &mut entry.pool.state {
            b.proceeds = None;
        }
        let call = part(ctx, &entry, o, h.timestamp.to())?;
        return Ok(Choice { entry, call });
    }
    Err(bad(ctx))
}
