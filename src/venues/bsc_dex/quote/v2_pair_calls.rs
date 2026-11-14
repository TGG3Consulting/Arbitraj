//! Файл: venues/bsc_dex/quote/v2_pair_calls.rs
//! Что делает: решение 280 (доработка по ревью) — семь вызовов eth_simulateV1 сверки пары V2-форка без роутера через
//! код ArbExecutor (чистая функция — тест сверяет их байт-в-байт с записанным запросом):
//!   0 getReserves пары (резервы блока) → 1 пара переводит вход сделки ПОДМЕНЁННОМУ ИСПОЛНИТЕЛЮ (from = пара, без
//!   подписи, validation = false) → 2 sync() пары (резерв входа = баланс) → 3 getReserves (резервы, по которым считает
//!   исполнитель) → 4 balanceOf(исполнитель) по токену входа — сколько ФАКТИЧЕСКИ пришло (у монеты с налогом на
//!   перевод меньше суммы) → 5 execute(вход, amountIn = 0, выход, 1, без срока, [шаг V2 пары, наша комиссия, вся доля])
//!   → 6 getReserves (резервы после обмена: сколько дошло до пары и сколько она отдала).
//! amountIn = 0 — штатная ветка контракта v1 без изменения кода (contracts/ArbExecutor.sol execute/_step): забора с
//! владельца нет, шаг берёт ВЕСЬ текущий баланс исполнителя (shareBps = 10000) — то есть фактически пришедшее; прежняя
//! сверка (перевод владельцу, approve и execute на запрошенную сумму) у монеты с налогом откатывалась в transferFrom и
//! исключала продажу таких монет с неверной причиной.
//! Кто вызывает: quote/v2_pair_quote.rs (calls), тесты.
//! От чего зависит: check/simulate.rs (Call), exec/executor/{abi, kind}.rs, pools/abi_v2v3.rs.
//! Настройки: нет (комиссия приходит параметром).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use super::super::check::simulate::Call;
use super::super::exec::executor::abi::ArbExecutor;
use super::super::exec::executor::kind::{self, ALL_BPS, Params, StepSpec};
use super::super::pools::abi_v2v3::IV2Pair;

alloy::sol! {
    function sync() external;
    function transfer(address to, uint256 value) external returns (bool);
    function balanceOf(address owner) external view returns (uint256);
}

/// Число вызовов сверки.
pub const CALLS: usize = 7;

/// Семь вызовов сверки (см. шапку). Принимает: пару, токены входа и выхода, комиссию шага (пипсы 1e-6), сумму, адреса
/// подменённого исполнителя и его владельца. Возвращает: вызовы. Ошибки (текст): шаг не кодируется (kind::encode).
pub fn build(
    pair: Address,
    (token_in, token_out): (Address, Address),
    fee_pips: u32,
    amount: U256,
    (executor, owner): (Address, Address),
) -> Result<Vec<Call>, String> {
    let step = kind::encode(&StepSpec {
        params: Params::V2 { fee_pips },
        pool: pair,
        token_in,
        token_out,
        share_bps: ALL_BPS,
    })?;
    let exec = ArbExecutor::executeCall {
        tokenIn: token_in,
        // 0 — забора с владельца нет: шаг берёт весь баланс исполнителя (пришедшее из пары после налога перевода).
        amountIn: U256::ZERO,
        tokenOut: token_out,
        minOut: U256::from(1u8),
        // Срок — не настройка: симуляция одна, в сеть ничего не уходит; «без срока» = наибольшее uint256.
        deadline: U256::MAX,
        steps: vec![step],
    };
    let c = |from: Address, to: Address, data: Vec<u8>| Call {
        from,
        to,
        input: Bytes::from(data),
    };
    let reserves = IV2Pair::getReservesCall {}.abi_encode();
    Ok(vec![
        c(owner, pair, reserves.clone()),
        c(
            pair,
            token_in,
            transferCall {
                to: executor,
                value: amount,
            }
            .abi_encode(),
        ),
        c(owner, pair, syncCall {}.abi_encode()),
        c(owner, pair, reserves.clone()),
        c(
            owner,
            token_in,
            balanceOfCall { owner: executor }.abi_encode(),
        ),
        c(owner, executor, exec.abi_encode()),
        c(owner, pair, reserves),
    ])
}
