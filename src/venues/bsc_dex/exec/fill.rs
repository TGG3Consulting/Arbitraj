//! Файл: venues/bsc_dex/exec/fill.rs
//! Что делает: факт обмена или перевода по квитанции — суммы из событий Transfer токенов: сколько ушло из кошелька
//! и сколько пришло на кошелёк (реальные суммы, с налогами и крючками) — для сверки плана и факта.
//! Кто вызывает: exec/trade.rs, wallet/transfer.rs.
//! От чего зависит: node/abi.rs (событие Transfer), send/track.rs (Receipt).
//! Настройки: нет.
//!
//! Transfer(address indexed from, address indexed to, uint256 value) — https://eips.ethereum.org/EIPS/eip-20 ;
//! «реальные суммы из логов Transfer → сверка с планом» — research/flow/bsc_dex.md, схема адаптера п. 8.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, U256};

use super::super::node::abi::IERC20;
use super::super::send::track::Receipt;

/// Сумма переводов токена по квитанции. Принимает: квитанцию, токен, «от кого» и/или «кому» (None — любой).
/// Возвращает: сумму value подходящих событий. Ошибок нет (чужие и неразборчивые логи пропускаются).
pub fn moved(r: &Receipt, token: Address, from: Option<Address>, to: Option<Address>) -> U256 {
    r.logs
        .iter()
        .filter(|l| l.address() == token)
        .filter_map(|l| IERC20::Transfer::decode_log_data(&l.inner.data).ok())
        .filter(|t| from.is_none_or(|f| t.from == f) && to.is_none_or(|x| t.to == x))
        .fold(U256::ZERO, |acc, t| acc.saturating_add(t.value))
}

/// Токен первого события Transfer «из кошелька» в квитанции. None — такого события нет.
pub fn token_from(r: &Receipt, wallet: Address) -> Option<Address> {
    r.logs.iter().find_map(|l| {
        let t = IERC20::Transfer::decode_log_data(&l.inner.data).ok()?;
        (t.from == wallet).then(|| l.address())
    })
}
