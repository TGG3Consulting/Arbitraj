//! Движение средств отдельного подписанного поручения внутри batch. Событие роутера и физические переводы сверяются вместе.
use super::super::{exec::fill::moved_logs, send::receipt::Receipt};
use super::wire::{IntentSwapExecuted, SwapIntent};
use alloy::{
    primitives::{Address, U256},
    sol_types::SolEvent,
};

#[derive(Debug, PartialEq, Eq)]
pub struct Fill {
    pub input_to_router: U256,
    pub input_from_path: U256,
    pub other_input_credit: U256,
    pub output_from_router: U256,
    pub other_output_credit: U256,
    /// Входит в исходную сумму, перечисленную плательщиком.
    pub fee_in_input: U256,
}

pub fn confirmed(order: &SwapIntent, router: Address, receipt: &Receipt) -> Option<Fill> {
    if receipt.status.to::<u64>() != 1
        || order.tokenIn == order.tokenOut
        || order.user != order.recipient
    {
        return None;
    }
    let mut start = 0;
    let mut found = None;
    for (end, log) in receipt.logs.iter().enumerate() {
        if log.address() != router
            || log.topics().first() != Some(&IntentSwapExecuted::SIGNATURE_HASH)
        {
            continue;
        }
        let ev = IntentSwapExecuted::decode_log_data(&log.inner.data).ok()?;
        let scope = &receipt.logs[start..end];
        start = end + 1;
        if ev.user != order.user || ev.nonce != order.nonce {
            continue;
        }
        if found.is_some()
            || (ev.tokenIn, ev.tokenOut, ev.recipient)
                != (order.tokenIn, order.tokenOut, order.recipient)
            || ev.amountIn != order.amountIn
            || ev.amountOut < order.minAmountOut
            || ev.fee >= ev.amountIn
        {
            return None;
        }
        let moved = |token, from, to| moved_logs(scope, token, from, to);
        let wallet = Some(order.user);
        let paid = moved(order.tokenIn, wallet, Some(router));
        let obtained = moved(order.tokenOut, Some(router), wallet);
        if paid != ev.amountIn
            || obtained != ev.amountOut
            || moved(order.tokenIn, wallet, None) != paid
            || !moved(order.tokenOut, wallet, None).is_zero()
        {
            return None;
        }
        let path_credit = order.path.iter().try_fold(U256::ZERO, |sum, hop| {
            sum.checked_add(moved(order.tokenIn, Some(hop.pool), wallet))
        })?;
        found = Some(Fill {
            input_to_router: paid,
            output_from_router: obtained,
            input_from_path: path_credit,
            other_input_credit: moved(order.tokenIn, None, wallet).checked_sub(path_credit)?,
            other_output_credit: moved(order.tokenOut, None, wallet).checked_sub(obtained)?,
            fee_in_input: ev.fee,
        });
    }
    found
}
