//! Расчёт EtherVista по реальной плате роутера в wei и резервам пары.
//! Полный вход включает плату при покупке. Полный выход при продаже уже за вычетом платы.
use super::{swap_cl::Swapped, v2};
use crate::core::types::EtherVistaState;
use alloy_primitives::{Address, U256};

pub(crate) fn pool_input(state: &EtherVistaState, token_in: Address, input: U256) -> Option<U256> {
    let debit = if token_in == state.wrapped {
        input.checked_sub(state.buy_fee)?
    } else {
        input
    };
    (!debit.is_zero()).then_some(debit)
}

pub(crate) fn wallet_output(
    state: &EtherVistaState,
    token_in: Address,
    gross: U256,
) -> Option<U256> {
    let net = if token_in == state.wrapped {
        gross
    } else {
        gross.checked_sub(state.sell_fee)?
    };
    (!net.is_zero()).then_some(net)
}

pub(crate) fn credited(
    state: &EtherVistaState,
    token_in: Address,
    zero_for_one: bool,
    input: U256,
) -> Option<U256> {
    let (balance, reserve) = if zero_for_one {
        (state.balance0, state.reserve0)
    } else {
        (state.balance1, state.reserve1)
    };
    let paid = pool_input(state, token_in, input)?;
    balance
        .checked_add(paid)?
        .checked_sub(reserve)
        .filter(|v| !v.is_zero())
}

pub(crate) fn quote(
    state: &EtherVistaState,
    token_in: Address,
    zero_for_one: bool,
    exact_in: bool,
    amount: U256,
) -> Option<Swapped> {
    if amount.is_zero() {
        return None;
    }
    let (rin, rout, bin, bout) = if zero_for_one {
        (
            state.reserve0,
            state.reserve1,
            state.balance0,
            state.balance1,
        )
    } else {
        (
            state.reserve1,
            state.reserve0,
            state.balance1,
            state.balance0,
        )
    };
    // Роутер выдаёт floor от резервов. Недостача хотя бы одной минимальной единицы
    // по стороне выхода нарушает K при любом размере: запас округления строго меньше единицы.
    if rin.is_zero() || rout.is_zero() || bout < rout {
        return None;
    }
    let buy = token_in == state.wrapped;
    let input = if exact_in {
        amount
    } else {
        let gross = if buy {
            amount
        } else {
            amount.checked_add(state.sell_fee)?
        };
        if gross >= rout {
            return None;
        }
        let required_credit = rin.checked_mul(gross)?.div_ceil(rout - gross);
        let debit = rin
            .checked_add(required_credit)?
            .saturating_sub(bin)
            .max(U256::ONE);
        if buy {
            debit.checked_add(state.buy_fee)?
        } else {
            debit
        }
    };
    let dx = credited(state, token_in, zero_for_one, input)?;
    let gross = v2::amount_out(dx, rin, rout, (U256::ONE, U256::ONE))?;
    let pre = rin.checked_add(dx)?;
    let post = bout.checked_sub(gross)?;
    if pre.bit_len() > 112
        || post.bit_len() > 112
        || pre.checked_mul(post)? < rin.checked_mul(rout)?
    {
        return None;
    }
    let net = wallet_output(state, token_in, gross)?;
    if !exact_in && net < amount {
        return None;
    }
    Some(Swapped {
        amount_in: input,
        amount_out: if exact_in { net } else { amount },
        complete: true,
    })
}

pub(crate) fn after(
    state: &EtherVistaState,
    token_in: Address,
    zero_for_one: bool,
    input: U256,
) -> Option<(Swapped, EtherVistaState)> {
    let fill = quote(state, token_in, zero_for_one, true, input)?;
    let debit = pool_input(state, token_in, input)?;
    let gross = if token_in == state.wrapped {
        fill.amount_out
    } else {
        fill.amount_out.checked_add(state.sell_fee)?
    };
    let mut next = state.clone();
    if zero_for_one {
        next.balance0 = next.balance0.checked_add(debit)?;
        next.balance1 = next.balance1.checked_sub(gross)?;
    } else {
        next.balance1 = next.balance1.checked_add(debit)?;
        next.balance0 = next.balance0.checked_sub(gross)?;
    }
    next.reserve0 = next.balance0;
    next.reserve1 = next.balance1;
    Some((fill, next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn uint(value: &Value) -> U256 {
        if let Some(v) = value.as_str() {
            v.parse().unwrap()
        } else {
            U256::from(value.as_u64().unwrap())
        }
    }

    #[test]
    fn both_archives_reproduce_payer_payout_fixed_fees_and_second_swap() {
        for file in [
            include_str!("../../../../tests/data/first_run_dex_2026-11-26/ethervista_base.json"),
            include_str!(
                "../../../../tests/data/first_run_dex_2026-11-26/ethervista_ethereum.json"
            ),
        ] {
            let directory: Value = serde_json::from_str(file).unwrap();
            let weth: Address = directory["weth"].as_str().unwrap().parse().unwrap();
            let sample = &directory["observations"][0];
            let pool = directory["registry"]
                .as_array()
                .unwrap()
                .iter()
                .find(|pool| pool["pool"] == sample["pool"])
                .unwrap();
            let weth0 = pool["tokens"][0] == directory["weth"];
            let token: Address = sample["token"].as_str().unwrap().parse().unwrap();
            let state = EtherVistaState {
                balance0: uint(&pool["reserves"][0]),
                balance1: uint(&pool["reserves"][1]),
                wrapped: weth,
                reserve0: uint(&pool["reserves"][0]),
                reserve1: uint(&pool["reserves"][1]),
                buy_fee: uint(&sample["fees"]["buy"]["native_wei"]),
                sell_fee: uint(&sample["fees"]["sell"]["native_wei"]),
            };
            assert!(quote(&state, weth, weth0, true, state.buy_fee).is_none());
            let mut positive = 0;
            let mut negative = 0;
            for row in sample["swaps"].as_array().unwrap() {
                let total = uint(&row["total_buy_eth"]);
                let tokens = uint(&row["buy"]["wallet_tokens"]);
                let (bought, next) = after(&state, weth, weth0, total).unwrap();
                assert_eq!((bought.amount_in, bought.amount_out), (total, tokens));
                let least = quote(&state, weth, weth0, false, tokens).unwrap().amount_in;
                assert!(least <= total);
                assert!(quote(&state, weth, weth0, true, least).unwrap().amount_out >= tokens);
                assert!(
                    quote(&state, weth, weth0, true, least - U256::ONE)
                        .is_none_or(|q| q.amount_out < tokens)
                );
                let gross = uint(&row["measured_gross_wei"]);
                let fill = quote(&next, token, !weth0, true, tokens);
                if gross <= state.sell_fee {
                    negative += 1;
                    assert!(fill.is_none());
                } else {
                    positive += 1;
                    let net = gross - state.sell_fee;
                    let (sold, end) = after(&next, token, !weth0, tokens).unwrap();
                    assert_eq!(fill.unwrap().amount_out, net);
                    assert_eq!(
                        sold.amount_out,
                        uint(&row["sell_same_wallet"]["wallet_native"])
                    );
                    let (end_weth, initial_weth) = if weth0 {
                        (end.reserve0, state.reserve0)
                    } else {
                        (end.reserve1, state.reserve1)
                    };
                    assert_eq!(end_weth, initial_weth + total - state.buy_fee - gross);
                    let least = quote(&next, token, !weth0, false, net).unwrap().amount_in;
                    assert!(least <= tokens);
                    assert!(
                        quote(&next, token, !weth0, true, least - U256::ONE)
                            .is_none_or(|q| q.amount_out < net)
                    );
                }
            }
            assert_eq!((positive, negative), (1, 1));
        }
    }

    #[test]
    fn inverse_does_not_overbuy_on_integer_boundary() {
        let weth = Address::repeat_byte(1);
        let other = Address::repeat_byte(2);
        let state = EtherVistaState {
            balance0: U256::from(100),
            balance1: U256::from(100),
            reserve0: U256::from(100),
            reserve1: U256::from(100),
            wrapped: weth,
            buy_fee: U256::from(7),
            sell_fee: U256::from(3),
        };
        assert_eq!(
            quote(&state, weth, true, false, U256::from(50))
                .unwrap()
                .amount_in,
            U256::from(107)
        );
        assert_eq!(
            quote(&state, other, false, false, U256::from(47))
                .unwrap()
                .amount_in,
            U256::from(100)
        );
        assert!(quote(&state, other, false, false, U256::from(97)).is_none());
    }
    #[test]
    fn donated_input_and_rebasing_shortfall_update_real_pair_balances() {
        let weth = Address::repeat_byte(1);
        let token = Address::repeat_byte(2);
        let mut s = EtherVistaState {
            wrapped: weth,
            buy_fee: U256::ZERO,
            sell_fee: U256::ZERO,
            reserve0: U256::from(100),
            reserve1: U256::from(100),
            balance0: U256::from(120),
            balance1: U256::from(100),
        };
        let (fill, next) = after(&s, weth, true, U256::ONE).unwrap();
        assert_eq!(fill.amount_out, U256::from(17));
        assert_eq!(
            (next.reserve0, next.reserve1),
            (U256::from(121), U256::from(83))
        );
        assert_eq!(
            quote(&s, weth, true, false, U256::from(17))
                .unwrap()
                .amount_in,
            U256::ONE
        );
        s.balance0 = U256::from(80);
        assert!(quote(&s, weth, true, true, U256::from(20)).is_none());
        assert_eq!(
            quote(&s, weth, true, false, U256::from(50))
                .unwrap()
                .amount_in,
            U256::from(120)
        );
        assert!(quote(&s, token, false, true, U256::from(500)).is_none());
    }
}
