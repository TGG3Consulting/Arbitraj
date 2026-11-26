//! Безналоговый эталон CandySwap для конкретной ноги eth_simulateV1.
//! Каждая нога читает текущие резервы, остатки ERC-20 и условия выплаты в своём EVM-состоянии.
use super::super::ctx::Ctx;
use super::super::math::candy;
use super::super::node::{abi::IERC20, multicall};
use super::super::pools::{abi_v2v3::IV2Pair, registry::Entry, registry_key};
use crate::core::types::CandySwapState;
use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256};

pub(super) fn request(ctx: &Ctx, e: &Entry, pool: Address) -> GatewayResult<(Address, Bytes)> {
    let PoolState::CandySwap(state) = &e.pool.state else {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "CandySwap: условия выплаты ещё не прочитаны",
        ));
    };
    let collector = state.receiver;
    if collector == pool
        || collector == super::super::wallet::signer::address(ctx)?
        || Some(collector) == super::super::exec::executor::address(ctx)?
    {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "CandySwap: получатель сбора участвует в пробном обмене",
        ));
    }
    let factory = ctx.dex_addr("dex.factory", e.family.id)?;
    let balance: Bytes = IERC20::balanceOfCall { owner: pool }.abi_encode().into();
    let calls = [
        (pool, IV2Pair::getReservesCall {}.abi_encode().into()),
        (e.pool.token0, balance.clone()),
        (e.pool.token1, balance),
        (pool, registry_key::selector("swapFee()")),
        (pool, Bytes::from_static(&candy::PAYOUT_RATE_SELECTOR)),
        (factory, registry_key::selector("feeTo()")),
    ];
    Ok((ctx.addr("dex.multicall3")?, multicall::encode(&calls)))
}

pub(super) fn expected(
    raw: &[u8],
    fee_pips: u32,
    receiver: Address,
    zero_in: bool,
    amount: U256,
) -> Option<U256> {
    let parts = multicall::decode(raw, 6).ok()?;
    let data = |i: usize| -> Option<&[u8]> { parts.get(i)?.as_ref().ok().map(AsRef::as_ref) };
    let word = |i| {
        let b = data(i)?;
        (b.len() == 32).then(|| U256::from_be_slice(b))
    };
    let reserves = IV2Pair::getReservesCall::abi_decode_returns_validate(data(0)?).ok()?;
    let fee = word(3)?;
    let payout = word(4)?;
    if fee.checked_mul(U256::from(100))? != U256::from(fee_pips)
        || fee >= U256::from(10000)
        || payout > U256::from(10000)
        || Address::abi_decode_validate(data(5)?).ok()? != receiver
    {
        return None;
    }
    let market = CandySwapState {
        reserves: [U256::from(reserves.reserve0), U256::from(reserves.reserve1)],
        balances: [word(1)?, word(2)?],
        payout_bps: payout.to::<u16>(),
        receiver,
    };
    let basis = U256::from(10000);
    candy::funded(&market, zero_in, (basis - fee, basis), true, amount)
        .map(|(fill, _)| fill.amount_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use multicall::IMulticall3::{Result as Item, aggregate3Call};
    struct Frame {
        reserves: [u64; 2],
        balances: [u64; 2],
        payout: u64,
    }
    fn data(frame: Frame, receiver: Address) -> Vec<u8> {
        let words = [
            (
                U256::from(frame.reserves[0]),
                U256::from(frame.reserves[1]),
                U256::ZERO,
            )
                .abi_encode(),
            U256::from(frame.balances[0]).abi_encode(),
            U256::from(frame.balances[1]).abi_encode(),
            U256::from(15).abi_encode(),
            U256::from(frame.payout).abi_encode(),
            receiver.abi_encode(),
        ];
        let values = words
            .into_iter()
            .map(|raw| Item {
                success: true,
                returnData: raw.into(),
            })
            .collect::<Vec<_>>();
        aggregate3Call::abi_encode_returns(&values)
    }
    #[test]
    fn each_probe_frame_uses_current_backing_and_payout() {
        let receiver = Address::repeat_byte(1);
        let amount = U256::from(1_000_000);
        let base = 1_000_000_000;
        let leg = |r, b, p| {
            data(
                Frame {
                    reserves: r,
                    balances: b,
                    payout: p,
                },
                receiver,
            )
        };
        for (balance, out) in [
            (base, 996_007),
            (base + 100_000, 1_095_499),
            (base - 100_000, 896_497),
        ] {
            let raw = leg([base; 2], [balance, base], 15);
            assert_eq!(
                expected(&raw, 1500, receiver, true, amount),
                Some(U256::from(out))
            );
        }
        // Нога продажи читает новый Sync покупки и изменение баланса/ставки перед продажей.
        let second = leg(
            [1_001_100_000, 998_902_856],
            [1_001_100_000, 998_952_856],
            30,
        );
        let dx = U256::from(400_000);
        let gross = U256::from(450_000u64) * U256::from(9985) * U256::from(1_001_100_000u64)
            / (U256::from(998_902_856u64) * U256::from(10_000)
                + U256::from(450_000u64) * U256::from(9985));
        assert_eq!(
            expected(&second, 1500, receiver, false, dx),
            Some(gross - gross * U256::from(30) / U256::from(10_000))
        );
        assert_eq!(expected(&second, 1400, receiver, false, dx), None);
        assert_eq!(expected(&second, 1500, Address::ZERO, false, dx), None);
        assert_eq!(expected(&[], 1500, receiver, false, dx), None);
    }
}
