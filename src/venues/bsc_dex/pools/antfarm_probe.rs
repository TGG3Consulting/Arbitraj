//! Снимок для пробы Antfarm. Обратная нога читает состояние после пробной покупки,
//! включая купленные ATF, дивиденды пары и обновление часового оракула.
use super::super::{ctx::Ctx, node::multicall, quote::white_state};
use super::antfarm::{IFarm, derived};
use super::registry::Entry;
use crate::core::types::{AntfarmBook, AntfarmMarket, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};

sol! { function getCurrentBlockTimestamp() external view returns(uint256); }

pub(in crate::venues::bsc_dex) async fn anchor(
    ctx: &Ctx,
    e: &Entry,
    number: u64,
) -> GatewayResult<(white_state::Head, Box<Entry>)> {
    let h = white_state::head(ctx, &format!("{number:#x}")).await?;
    let mut entry = Box::new(e.clone());
    let b=super::antfarm_book::read(ctx,e.family,e.pool.reference.address(),(e.coin,e.usdt),&h.selector()).await?;
    entry.pool.state=PoolState::Antfarm(Box::new(b));
    entry.pool.block = number;
    Ok((h, entry))
}

pub(in crate::venues::bsc_dex) fn uses_funding_pair(e: &Entry, address: Address) -> bool {
    let PoolState::Antfarm(book) = &e.pool.state else {
        return false;
    };
    e.pool.reference.address() == address
        || book
            .funding
            .iter()
            .any(|(m, _)| derived(m.tokens, m.tier) == address)
}

fn reads(m: &AntfarmMarket, calls: &mut Vec<(Address, Bytes)>, project: bool) {
    let pool = derived(m.tokens, m.tier);
    let bal: Bytes = IFarm::balanceOfCall { account: pool }.abi_encode().into();
    calls.extend([
        (pool, IFarm::getReservesCall {}.abi_encode().into()),
        (pool, IFarm::antfarmTokenReserveCall {}.abi_encode().into()),
        (pool, IFarm::totalSupplyCall {}.abi_encode().into()),
        (m.tokens[0], bal.clone()),
        (m.tokens[1], bal.clone()),
        (m.atf, bal),
    ]);
    if let Some(oracle) = m.oracle_address {
        if m.oracle.is_some() || project {
            calls.push((oracle, IFarm::price1AverageCall {}.abi_encode().into()));
        }
        if project {
            calls.extend([
                (pool, IFarm::price1CumulativeLastCall {}.abi_encode().into()),
                (
                    oracle,
                    IFarm::price1CumulativeLastCall {}.abi_encode().into(),
                ),
                (oracle, IFarm::blockTimestampLastCall {}.abi_encode().into()),
                (oracle, IFarm::firstUpdateCallCall {}.abi_encode().into()),
            ]);
        }
    }
}

pub(in crate::venues::bsc_dex) fn request(ctx: &Ctx, e: &Entry) -> GatewayResult<(Address, Bytes)> {
    let PoolState::Antfarm(b) = &e.pool.state else {
        return Err(ctx.err(ErrorCode::NoRoute, "Antfarm: нет состояния рынков комиссии"));
    };
    let mc = ctx.addr("dex.multicall3")?;
    let mut calls = vec![(mc, getCurrentBlockTimestampCall {}.abi_encode().into())];
    reads(&b.market, &mut calls, false);
    for (fund, _) in &b.funding {
        reads(fund, &mut calls, fund.tier == 10);
    }
    Ok((mc, multicall::encode(&calls)))
}

type Part = Result<Bytes, String>;
fn read<C: SolCall>(rows: &mut std::slice::Iter<'_, Part>, _: C) -> Option<C::Return> {
    C::abi_decode_returns_validate(rows.next()?.as_ref().ok()?).ok()
}
fn observed(
    m: &mut AntfarmMarket,
    rows: &mut std::slice::Iter<'_, Part>,
    now: u32,
    project: bool,
) -> Option<Option<U256>> {
    let r = read(rows, IFarm::getReservesCall {})?;
    m.reserves = [U256::from(r.x), U256::from(r.y)];
    m.dividends = read(rows, IFarm::antfarmTokenReserveCall {})?;
    m.lp_supply = read(rows, IFarm::totalSupplyCall {})?;
    for b in &mut m.balances {
        *b = read(
            rows,
            IFarm::balanceOfCall {
                account: Address::ZERO,
            },
        )?;
    }
    if m.oracle.is_none() && !project {
        return Some(None);
    }
    m.oracle_address?;
    let avg = U256::from(read(rows, IFarm::price1AverageCall {})?);
    if let Some((_, v)) = m.oracle.as_mut() {
        *v = avg;
    }
    if !project {
        return Some(None);
    }
    let cumulative = read(rows, IFarm::price1CumulativeLastCall {})?;
    let base = read(rows, IFarm::price1CumulativeLastCall {})?;
    let elapsed = now.wrapping_sub(read(rows, IFarm::blockTimestampLastCall {})?);
    let first = read(rows, IFarm::firstUpdateCallCall {})?;
    if elapsed < 3600 && !first {
        return Some(Some(avg));
    }
    if elapsed == 0 || m.reserves.contains(&U256::ZERO) {
        return Some(None);
    }
    let unit: U256 = (m.reserves[0] << 112) / m.reserves[1];
    let next = cumulative.checked_add(unit * U256::from(now.wrapping_sub(r.last)));
    Some(
        next.map(|n| {
            (n.wrapping_sub(base) / U256::from(elapsed)) & ((U256::ONE << 224) - U256::ONE)
        }),
    )
}
fn snapshot(book: &AntfarmBook, raw: &[u8]) -> Option<AntfarmBook> {
    let count = |m: &AntfarmMarket, project: bool| {
        6 + usize::from(m.oracle_address.is_some() && (m.oracle.is_some() || project))
            + 4 * usize::from(project && m.oracle_address.is_some())
    };
    let n = 1
        + count(&book.market, false)
        + book
            .funding
            .iter()
            .map(|(m, _)| count(m, m.tier == 10))
            .sum::<usize>();
    let data = multicall::decode(raw, n).ok()?;
    let mut rows = data.iter();
    let now = read(&mut rows, getCurrentBlockTimestampCall {})?.wrapping_to::<u32>();
    let mut result = book.clone();
    observed(&mut result.market, &mut rows, now, false)?;
    for (m, rate) in &mut result.funding {
        *rate = observed(m, &mut rows, now, m.tier == 10)?;
    }
    result.funding.retain(|(m, r)| m.tier != 10 || r.is_some());
    rows.next().is_none().then_some(result)
}
pub(in crate::venues::bsc_dex) fn expected(
    book: &AntfarmBook,
    raw: &[u8],
    source: Address,
    amount: U256,
) -> Option<U256> {
    let fill=snapshot(book,raw)?.swap(source,true,amount)?;
    fill.complete.then_some(fill.amount_out)
}
pub(in crate::venues::bsc_dex) fn after(
    ctx: &Ctx,
    e: &Entry,
    raw: &[u8],
) -> GatewayResult<Box<Entry>> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Antfarm: не прочитаны резервы, средства на комиссию или оракул после обмена",
        )
    };
    let PoolState::Antfarm(book) = &e.pool.state else {
        return Err(bad());
    };
    let mut next = Box::new(e.clone());
    next.pool.state = PoolState::Antfarm(Box::new(snapshot(book, raw).ok_or_else(bad)?));
    Ok(next)
}
