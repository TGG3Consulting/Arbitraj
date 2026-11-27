//! Проба налога Surge с собственными средствами. Вторая нога использует следующий блок,
//! поскольку сам токен запрещает купившему адресу продавать SRG в том же блоке.
use super::{
    probe::{Measured, SellMiss},
    probe_calls as pc, probe_tiny,
    simulate::{self, Base, Call, CallResult},
};
use crate::venues::bsc_dex::exec::vista_wire::{Form, IVistaFlow};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::rpc,
        pools::{registry::Entry, surge},
        quote::{
            surge_pricing,
            white_state::Head,
        },
        wallet::approve,
    },
};
use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, U64, U256, keccak256};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Frame {
    number: U64,
    hash: B256,
    parent_hash: B256,
    calls: Vec<CallResult>,
}
fn word(r: &CallResult) -> Option<U256> {
    (r.ok() && r.return_data.len() == 32).then(|| U256::from_be_slice(&r.return_data))
}
fn block(rows: &[Call], values: &[(usize, U256)], number: u64, time: u64) -> Value {
    simulate::params(
        rows,
        values,
        &[],
        Base {
            number,
            time: Some(time),
        },
    )[0]["blockStateCalls"][0]
        .take()
}
struct Leg {
    calls: Vec<Call>,
    values: Vec<(usize, U256)>,
    swap: usize,
    paid: usize,
}
impl Leg {
    fn gas(&self, results: &[CallResult]) -> (u64, u64) {
        (
            results[self.swap].gas_used.to(),
            results[2..self.swap]
                .iter()
                .map(|r| r.gas_used.to::<u64>())
                .sum(),
        )
    }
    fn outcome(
        &self,
        results: &[CallResult],
        market: Address,
        bought: bool,
    ) -> Option<(U256, U256)> {
        let settled = results.get(self.swap)?;
        let swap = if bought {
            IVistaFlow::Bought::SIGNATURE_HASH
        } else {
            IVistaFlow::Sold::SIGNATURE_HASH
        };
        if results.iter().take(self.swap + 3).any(|r| !r.ok())
            || settled
                .logs
                .iter()
                .filter(|l| l.address == market && l.topics.first() == Some(&swap))
                .count()
                != 1
        {
            return None;
        }
        let debit = word(&results[0])?.checked_sub(word(&results[self.swap + 1])?)?;
        let credit = word(&results[self.swap + 2])?.checked_sub(word(&results[1])?)?;
        Some((debit, credit))
    }
}
pub(super) async fn run(
    ctx: &Ctx,
    e: &Entry,
    h: Head,
    wallet: Address,
    budget: U256,
    min: Option<U256>,
) -> GatewayResult<Result<Measured, String>> {
    let at = h.selector();
    let err = |s: &str| ctx.err(ErrorCode::NoRoute, format!("Surge: {s}"));
    let state = surge::market(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.usdt, e.coin),
        &at,
    )
    .await?;
    let native = e.family.executor_kind.is_none()
        || (e.usdt == state.wrapped && ctx.text("venue.quote_asset").is_ok_and(|t| super::super::exec::native::cash(ctx,&t,e.usdt)))
        || (e.coin == state.wrapped && super::super::exec::native::cash(ctx,&e.ticker,e.coin));
    let multi = if native {
        ctx.addr("dex.multicall3")?
    } else {
        Address::ZERO
    };
    alloy::sol! {function getEthBalance(address holder) external view returns(uint256);}
    let read = |token, holder| {
        if native && token == state.wrapped {
            pc::call(wallet, multi, getEthBalanceCall { holder }.abi_encode())
        } else {
            pc::balance(wallet, token, holder)
        }
    };
    let t = h.timestamp.to::<u64>();
    let n = h.number.to::<u64>();
    let first = Base::next_time(ctx.chain_id(), t);
    let second = Base::next_time(ctx.chain_id(), first);
    let cash = e.usdt;
    let coin = e.coin;
    let expected = state
        .swap(cash, true, budget)
        .ok_or_else(|| err("пробный объём или касса недоступны"))?
        .amount_out;
    if let Some(m) = min
        && let Some(reason) = probe_tiny::buy(ctx, e, expected, m)
    {
        return Err(reason);
    }
    let quarter = expected / U256::from(4);
    let sale = expected - expected / U256::from(2) - quarter;
    if quarter.is_zero() {
        return Err(err("малая выплата для проверки перевода"));
    }
    let mut post = state.clone();
    if cash == state.wrapped {
        post.stock -= expected;
        post.liquidity = post
            .liquidity
            .checked_add(
                budget
                    .checked_mul(U256::from(state.buy_mul))
                    .ok_or_else(|| err("переполнение"))?
                    / U256::from(100),
            )
            .ok_or_else(|| err("переполнение"))?;
        post.cash = post
            .cash
            .checked_add(budget)
            .ok_or_else(|| err("переполнение"))?;
        if state.trader==wallet {
            post.capacity = post.capacity.saturating_sub(expected);
        }
    } else {
        post.stock = post
            .stock
            .checked_add(budget)
            .ok_or_else(|| err("переполнение"))?;
        post.liquidity -= state
            .gross(cash, budget)
            .ok_or_else(|| err("не установлена валовая выплата"))?;
        post.cash -= expected;
        if state.trader==wallet {
            post.capacity = post.capacity.saturating_add(budget);
        }
    }
    let want = post
        .swap(coin, true, sale)
        .ok_or_else(|| err("после покупки обратная нога не покрыта"))?
        .amount_out;
    let leg = |input, output, amount, end| {
        let (target, wire) = surge_pricing::wire(ctx, e, &state, input, native)?;
        let mut rows = vec![read(input, wallet), read(output, wallet)];
        if matches!(wire.form,Form::SurgeToken|Form::SurgeModuleEthOut) {
            for (token, data) in approve::fork_all(ctx, input, target, amount)? {
                rows.push(Call {
                    from: wallet,
                    to: token,
                    input: data,
                });
            }
        }
        let swap = rows.len();
        rows.push(Call {
            from: wallet,
            to: target,
            input: wire.data(input, output, amount, U256::ONE, wallet, end),
        });
        rows.extend([read(input, wallet), read(output, wallet)]);
        let value = wire.value(amount);
        Ok::<_, crate::gateway::errors::GatewayError>(Leg {
            calls: rows,
            values: (!value.is_zero())
                .then_some((swap, value))
                .into_iter()
                .collect(),
            swap,
            paid: 0,
        })
    };
    let validity = ctx.int("dex.deadline_s")?;
    let buy = leg(cash, coin, budget, U256::from(t.saturating_add(validity)))?;
    let mut sell = leg(coin, cash, sale, U256::from(first.saturating_add(validity)))?;
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    let cash_before: U256 = if native && cash == state.wrapped {
        rpc::call(ctx, "eth_getBalance", json!([wallet, at])).await?
    } else {
        super::super::pools::dagrow::balance(ctx, cash, wallet, &at).await?
    };
    if cash_before < budget {
        return Err(ctx.err(
            ErrorCode::InsufficientBalance,
            "Surge: недостаточно собственных средств для пробы",
        ));
    }
    sell.calls.push(read(coin, empty));
    sell.paid = sell.calls.len();
    if native && coin == state.wrapped {
        sell.values.push((sell.paid, quarter));
        sell.calls.push(pc::call(wallet, empty, vec![]));
    } else {
        sell.calls.push(pc::transfer(wallet, coin, empty, quarter));
    }
    sell.calls.push(read(coin, empty));
    let frame = vec![
        block(&buy.calls, &buy.values, n, first),
        block(&sell.calls, &sell.values, n + 1, second),
    ];
    let result: Vec<Frame> = rpc::call(
        ctx,
        "eth_simulateV1",
        json!([{"blockStateCalls":frame,"validation":false},at]),
    )
    .await?;
    let [a, b] = <&[Frame; 2]>::try_from(result.as_slice()).map_err(|_| err("неполные блоки"))?;
    if a.parent_hash != h.hash
        || b.parent_hash != a.hash
        || a.number.to::<u64>() != n + 1
        || b.number.to::<u64>() != n + 2
        || a.calls.len() != buy.calls.len()
        || b.calls.len() != sell.calls.len()
    {
        return Err(err("высота или состав ответа отличается"));
    }
    let Some((debit, paid)) = buy.outcome(&a.calls, state.token, cash == state.wrapped) else {
        return Err(err("покупка, разрешение или измерение не подтверждены"));
    };
    if debit != budget || paid != expected {
        return Err(err("покупка расходится с проверенным курсом и кошельком"));
    }
    let transfer = b
        .calls
        .get(sell.paid)
        .filter(|r| r.ok())
        .ok_or_else(|| err("пробный перевод не прошёл"))?;
    let moved = word(&b.calls[sell.paid + 1])
        .and_then(|v| v.checked_sub(word(&b.calls[sell.paid - 1])?))
        .ok_or_else(|| err("не подтверждено поступление перевода"))?;
    let result = match sell.outcome(&b.calls, state.token, coin == state.wrapped) {
        Some((spent, got)) if spent == sale && got > U256::ZERO => Ok((want, got)),
        _ => Err(SellMiss {
            reverted: !b.calls[sell.swap].ok(),
            why: b.calls[sell.swap].why(),
        }),
    };
    let (gb, ga) = buy.gas(&a.calls);
    let gs = result.is_ok().then(|| sell.gas(&b.calls));
    Ok(Ok(Measured {
        buy_expected: expected,
        buy_got: paid,
        sell: result,
        transfer_sent: quarter,
        transfer_got: moved,
        gas_buy: gb,
        gas_approve_buy: ga,
        gas_sell: gs,
        gas_transfer: transfer.gas_used.to(),
    }))
}
