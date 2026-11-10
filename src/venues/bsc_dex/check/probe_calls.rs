//! Файл: venues/bsc_dex/check/probe_calls.rs
//! Что делает: вызовы пробного обмена для eth_simulateV1 — переводы, разрешения и балансы ERC-20.
//! Кто вызывает: check/probe.rs.
//! Там же: хвост второй симуляции пробы — продажа, затем пробный перевод (sell_tail, решение 252); итог пробной продажи — пара «квотер / пришло» или почему налог продажи не измерен (sell_result, решение 146),
//! откат шага пробной покупки — вердикт по монете или помеха (buy_revert; срок транзакции вышел — помеха, у Universal Router и у исполнителя).
//! От чего зависит: check/simulate.rs (Call), check/probe.rs (SellMiss), node/abi.rs, quote/quoter_calls.rs, quote/quoter_revert.rs.
//! Настройки: нет.
//!
//! ERC-20 — https://eips.ethereum.org/EIPS/eip-20 .

use alloy::sol_types::{Revert, SolCall, SolError};
use alloy_primitives::{Address, Bytes, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::exec::ur;
use super::super::node::abi::IERC20;
use super::super::pools::registry::Entry;
use super::super::quote::{quoter_calls, quoter_revert};
use super::super::wallet::approve::Approval;
use super::probe::SellMiss;
use super::simulate::{Call, CallResult};

/// Вызов from → to с данными. Ошибок нет.
pub fn call(from: Address, to: Address, data: Vec<u8>) -> Call {
    Call {
        from,
        to,
        input: Bytes::from(data),
    }
}

/// transfer токена. Ошибок нет.
pub fn transfer(from: Address, token: Address, to: Address, value: U256) -> Call {
    call(from, token, IERC20::transferCall { to, value }.abi_encode())
}

/// approve токена. Ошибок нет.
pub fn approve(from: Address, token: Address, spender: Address, value: U256) -> Call {
    call(
        from,
        token,
        IERC20::approveCall { spender, value }.abi_encode(),
    )
}

/// balanceOf. Ошибок нет.
pub fn balance(from: Address, token: Address, owner: Address) -> Call {
    call(from, token, IERC20::balanceOfCall { owner }.abi_encode())
}

/// Откат шага пробной покупки: вердикт по монете или помеха окружения. Шаги до квотера (перевод USDT из пула-источника,
/// разрешения на USDT) монету не касаются; откат квотера «не хватает ликвидности» (quote/quoter_revert.rs) — свойство
/// пула на эту сумму. Это — Err NoRoute: проверка не выполнена, монета не в чёрный список.
/// Принимает: номер шага, итог, номер шага квотера. Возвращает: Ok(причина вердикта) или Err.
pub fn buy_revert(ctx: &Ctx, i: usize, r: &CallResult, quoter: usize) -> GatewayResult<String> {
    let why = format!("шаг {i} пробной покупки откатился: {}", r.why());
    let msg = r
        .error
        .as_ref()
        .map(|e| e.message.as_str())
        .unwrap_or_default();
    // Срок транзакции вышел (TransactionDeadlinePassed) — не свойство монеты: не проверено, повтор (не чёрный список).
    if late(r) {
        let m = format!(
            "{why} — TransactionDeadlinePassed: срок транзакции вышел к симулированному блоку (время головы + 1 с, check/sim_base.rs) — проверьте dex.deadline_s и свежесть узла"
        );
        return Err(ctx.err(ErrorCode::NoRoute, m));
    }
    let env =
        i < quoter || (i == quoter && quoter_revert::classify(msg, Some(&r.return_data)).is_some());
    if env {
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    Ok(why)
}

/// Откат «срок транзакции вышел» (TransactionDeadlinePassed Universal Router или `require(…, "deadline")` своего
/// исполнителя, contracts/ArbExecutor.sol execute): не вердикт по монете — голова устарела к симуляции или
/// dex.deadline_s меньше шага времени симулированного блока (1 с, решение 127), проверка «не проверено».
/// Принимает: результат вызова. Ошибок нет.
pub fn late(r: &CallResult) -> bool {
    let msg = r
        .error
        .as_ref()
        .map(|e| e.message.as_str())
        .unwrap_or_default();
    // Откат исполнителя — Error(string) «deadline» (Solidity require; разбор — alloy Revert): ровно эта строка.
    let exec_late = Revert::abi_decode(&r.return_data).is_ok_and(|e| e.reason == "deadline");
    !r.ok() && (ur::deadline_passed(&r.return_data, msg) || exec_late)
}

/// uint256 из ответа balanceOf. None — не по ABI.
pub fn u256(r: &CallResult) -> Option<U256> {
    IERC20::balanceOfCall::abi_decode_returns(&r.return_data).ok()
}

/// Итог пробной продажи (решение 146). Принимает: пул, четыре итога подряд — квотер на продажу остатка, баланс
/// встречного токена кошелька до продажи, продажа, баланс после; прошла ли продажа с её разрешениями.
/// Возвращает: Ok((ответ квотера, прирост баланса)) или Err(SellMiss) — налог продажи НЕ ИЗМЕРЕН: продажа откатилась
/// (reverted = true, вердикт по пулу) или квотер не ответил / ответ не по ABI (помеха). Ошибок иных нет.
pub fn sell_result(e: &Entry, r: &[CallResult], sold: bool) -> Result<(U256, U256), SellMiss> {
    let miss = |reverted, why: String| SellMiss { reverted, why };
    let [quote, before, swap, after] = r else {
        return Err(miss(false, "итогов пробной продажи не четыре".into()));
    };
    if !sold {
        return Err(miss(
            true,
            format!(
                "пробная продажа или её разрешения откатились: {}",
                swap.why()
            ),
        ));
    }
    if !quote.ok() {
        return Err(miss(
            false,
            format!("квотер продажи не ответил: {}", quote.why()),
        ));
    }
    match (
        quoter_calls::decode(e, &quote.return_data),
        u256(before),
        u256(after),
    ) {
        (Some(expected), Some(b0), Some(b1)) => Ok((expected, b1.saturating_sub(b0))),
        _ => Err(miss(
            false,
            "ответ квотера продажи или баланса не по ABI".into(),
        )),
    }
}

/// Номера вызовов хвоста пробы в общем списке (sell_tail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tail {
    /// Квотер на продажу; за ним подряд: баланс встречного токена до, продажа, баланс после (sell_result).
    pub quote: usize,
    /// Пробный перевод монеты на пустой адрес; за ним — баланс пустого адреса.
    pub transfer: usize,
}

/// Хвост второй симуляции пробы (решение 252): разрешения продажи → квотер на продажу → баланс встречного токена →
/// продажа → баланс встречного токена → перевод четверти монеты на пустой адрес → его баланс. Перевод — ПОСЛЕ продажи:
/// настоящая продажа идёт без перевода монеты перед ней в том же блоке, а у монеты с отражением (reflection) перевод
/// поднимает balanceOf пары V2 выше резерва — вход продажи (balanceOf − резерв) был бы больше настоящего.
/// Принимает: список вызовов (дописывается), (кошелёк, монета, встречный токен, пустой адрес), сумму перевода,
/// разрешения продажи, квотер (адрес, данные), продажу (роутер, данные). Возвращает: номера вызовов. Ошибок нет.
pub fn sell_tail(
    list: &mut Vec<Call>,
    (wallet, coin, counter, empty): (Address, Address, Address, Address),
    quarter: U256,
    approvals: &[Approval],
    (quoter, quote_data): (Address, Bytes),
    (router, sell_data): (Address, Bytes),
) -> Tail {
    list.extend(approvals.iter().map(|(to, d)| Call {
        from: wallet,
        to: *to,
        input: d.clone(),
    }));
    let quote = list.len();
    list.extend([
        Call {
            from: wallet,
            to: quoter,
            input: quote_data,
        },
        balance(wallet, counter, wallet),
        Call {
            from: wallet,
            to: router,
            input: sell_data,
        },
        balance(wallet, counter, wallet),
    ]);
    let at = list.len();
    list.extend([
        transfer(wallet, coin, empty, quarter),
        balance(wallet, coin, empty),
    ]);
    Tail {
        quote,
        transfer: at,
    }
}
