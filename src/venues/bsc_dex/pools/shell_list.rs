//! Файл: venues/bsc_dex/pools/shell_list.rs
//! Что делает: решение 486 — пулы Shell Protocol из списка dex.stable_pools как источник справочника (как Saddle /
//! Curve 2020 у решения 292, stable_list.rs): у пула нет адресов токенов — только id сторон в учёте (xToken / yToken),
//! поэтому монеты пула — те монеты подписки и встречные токены, чей keccak256(токен, 0) равен id стороны (нативная
//! монета учёта — обёртка dex.native_wrapped, если она среди встречных / монет). Дальше — обычная проверка
//! (verify_shell.rs).
//! Кто вызывает: pools/stable_list.rs (token_calls, sort_with).
//! От чего зависит: abi_shell.rs, shell_sides.rs, node/call.rs (решение ABI).
//! Настройки: нет (список dex.stable_pools и dex.native_wrapped читает stable_list.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use super::abi_shell::IShellPool as P;
use super::shell_sides;

/// Вызовы «стороны пула»: xToken(), yToken(). Ошибок нет.
pub fn calls(pool: Address) -> Vec<(Address, Bytes)> {
    vec![
        (pool, P::xTokenCall {}.abi_encode().into()),
        (pool, P::yTokenCall {}.abi_encode().into()),
    ]
}

/// Токены пула среди кандидатов (монеты подписки и встречные). Принимает: ответы calls, кандидатов, обёртку.
/// Возвращает: найденные токены сторон (обе стороны должны найтись) или причину (ответ не по ABI, сторона не среди
/// кандидатов).
pub fn tokens(
    answers: &[Result<Bytes, String>],
    candidates: &[Address],
    wrapped: Option<Address>,
) -> Result<Vec<Address>, String> {
    let [Ok(x), Ok(y)] = answers else {
        return Err("пул не ответил на xToken() / yToken() (откат или не 2 ответа)".to_string());
    };
    let x = P::xTokenCall::abi_decode_returns(x).map_err(|e| format!("xToken() не по ABI: {e}"))?;
    let y = P::yTokenCall::abi_decode_returns(y).map_err(|e| format!("yToken() не по ABI: {e}"))?;
    let sx = shell_sides::side(x, candidates, wrapped);
    let sy = shell_sides::side(y, candidates, wrapped);
    match (sx, sy) {
        (Some(a), Some(b)) if a.0 != b.0 => Ok(vec![a.0, b.0]),
        _ => Err(format!(
            "стороны пула в учёте {x:#x} / {y:#x} не среди монет подписки и встречных токенов"
        )),
    }
}
