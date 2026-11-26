//! Независимая проверка CandySwap при расхождении сохранённых резервов и реальных балансов.
//! Деньги для пробы приходят извне пула. Его исходные резервы и остатки сохраняются до swap.
use super::{v2_pair_quote, white_state};
use crate::core::types::CandySwapState;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    check::simulate::{self, Call},
    ctx::Ctx,
    math::candy,
    node::{abi::IERC20, call},
    pools::{abi_v2v3::IV2Pair, registry::Entry},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

fn tx(from: Address, to: Address, input: impl Into<Bytes>) -> Call {
    Call {
        from,
        to,
        input: input.into(),
    }
}

pub(super) async fn quote(
    ctx: &Ctx,
    e: &Entry,
    state: &CandySwapState,
    input: Address,
    amount: U256,
    tag: &str,
) -> GatewayResult<U256> {
    let head = white_state::head(ctx, tag).await?;
    let pool = v2_pair_quote::pair(ctx, e)?;
    let owner = v2_pair_quote::owner();
    let executable = v2_pair_quote::executor();
    let i = usize::from(input != e.pool.token0);
    let output = if i == 0 { e.pool.token1 } else { e.pool.token0 };
    let bad = |why: &str| ctx.err(ErrorCode::NoRoute, format!("CandySwap: {why}"));
    let payer = super::super::wallet::signer::address(ctx)?;
    if state.balances[i] > state.reserves[i] {
        return Err(bad(
            "для этого состояния требуется модуль с проверкой собственного входа",
        ));
    }
    let balance = |token: Address, who: Address| {
        tx(
            owner,
            token,
            IERC20::balanceOfCall { owner: who }.abi_encode(),
        )
    };
    let (template, over, _) = v2_pair_quote::calls(ctx, e, input, amount)?;
    let execute = template
        .get(5)
        .ok_or_else(|| bad("не построен шаг V5"))?
        .clone();
    let sim = [
        tx(owner, pool, IV2Pair::getReservesCall {}.abi_encode()),
        balance(input, pool),
        balance(output, pool),
        balance(output, owner),
        tx(
            owner,
            pool,
            Bytes::from_static(&candy::PAYOUT_RATE_SELECTOR),
        ),
        tx(
            owner,
            pool,
            super::super::pools::registry_key::selector("swapFee()"),
        ),
        tx(
            payer,
            input,
            IERC20::transferCall {
                to: executable,
                value: amount,
            }
            .abi_encode(),
        ),
        balance(input, executable),
        execute.clone(),
        balance(output, pool),
        balance(input, pool),
        balance(output, owner),
    ];
    let rows = simulate::run_anchored(
        ctx,
        &sim,
        &[],
        std::slice::from_ref(&over),
        head.number.to::<u64>().into(),
        Some(head.hash),
    )
    .await?;
    if let Some((at, r)) = rows.iter().enumerate().find(|(_, r)| !r.ok()) {
        return Err(bad(&format!(
            "проверка с кошельком плательщика, шаг {at}: {}",
            r.why()
        )));
    }
    let word = |n: usize| {
        let b = &rows[n].return_data;
        if b.len() != 32 {
            return Err(bad("нет 32-байтового результата"));
        }
        Ok(U256::from_be_slice(b))
    };
    let reserves = call::decode::<IV2Pair::getReservesCall>(&rows[0].return_data)?;
    if state.reserves != [U256::from(reserves.reserve0), U256::from(reserves.reserve1)]
        || state.balances[i] != word(1)?
        || state.balances[1 - i] != word(2)?
        || U256::from(state.payout_bps) != word(4)?
        || word(5)?.checked_mul(U256::from(100)) != e.pool.fee_pips.map(U256::from)
    {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "CandySwap: состояние пробы отличается от снимка котировки",
        ));
    }
    if word(7)? != amount || word(10)?.checked_sub(word(1)?) != Some(amount) {
        return Err(bad("с кошелька плательщика не поступил полный объём пробы"));
    }
    let gross = word(2)?
        .checked_sub(word(9)?)
        .ok_or_else(|| bad("не подтверждён расход пула"))?;
    let paid = word(11)?
        .checked_sub(word(3)?)
        .ok_or_else(|| bad("не подтверждён приход кошелька"))?;
    let before_coin_tax =
        candy::net(gross, state.payout_bps).ok_or_else(|| bad("некорректная ставка выплаты"))?;
    if paid.is_zero() || paid > before_coin_tax || word(8)? != paid {
        return Err(bad("выплата V5 не совпадает с дельтой кошелька"));
    }
    Ok(before_coin_tax)
}
