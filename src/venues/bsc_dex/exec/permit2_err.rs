//! Файл: venues/bsc_dex/exec/permit2_err.rs
//! Что делает: решение 303 — расшифровка отката Permit2 в тексте журнала понятными словами: AllowanceExpired (срок
//! разрешения Permit2 → Universal Router истёк, с датой срока) и InsufficientAllowance (разрешённой суммы не хватает).
//! Живой случай FOREST 15.11.2026: «execution reverted: 0xd81b2f2e…6af5f951» — без расшифровки причина не читалась.
//! Кто вызывает: exec/precheck_sim.rs (judge — проба перед отправкой прямого и умного пути), send/reject_log.rs (строка
//! отказа канала send.channel_rejected — дополнение 303).
//! От чего зависит: alloy (селекторы по сигнатурам), chrono (дата срока).
//! Настройки: нет.
//!
//! Источник: Permit2, src/interfaces/IAllowanceTransfer.sol, строки 10-16 —
//! https://github.com/Uniswap/permit2/blob/cc56ad0f3439c502c246fc5cfcc3db92bb8b7219/src/interfaces/IAllowanceTransfer.sol#L10-L16
//! («error AllowanceExpired(uint256 deadline)» — «Thrown when an allowance on a token has expired»;
//! «error InsufficientAllowance(uint256 amount)» — «Thrown when an allowance on a token has been depleted»).
//! Permit2 PancakeSwap — тот же интерфейс (wallet/approve.rs). Данные отката custom error — 4 байта селектора и
//! аргумент uint256 словом 32 байта (Solidity ABI).

use alloy::sol_types::SolError;
use alloy_primitives::{U256, hex};
use chrono::DateTime;

alloy::sol! {
    interface IPermit2Errors {
        error AllowanceExpired(uint256 deadline);
        error InsufficientAllowance(uint256 amount);
    }
}

pub use IPermit2Errors::{AllowanceExpired, InsufficientAllowance};

/// Понятная расшифровка отката Permit2 по данным отката или по тексту узла («execution reverted: 0x…» — так узел
/// пишет custom error, returnData может быть пуст). Принимает: данные отката, текст ошибки узла.
/// Возвращает: строку для журнала или None — откат не Permit2 AllowanceExpired / InsufficientAllowance. Ошибок нет.
pub fn explain(data: &[u8], message: &str) -> Option<String> {
    if let Some(arg) = arg_of(AllowanceExpired::SELECTOR, data, message) {
        let when = arg
            .and_then(|d| u64::try_from(d).ok())
            .and_then(|s| i64::try_from(s).ok())
            .and_then(|s| DateTime::from_timestamp(s, 0))
            .map_or(String::new(), |t| {
                format!(" {}", t.format("%d.%m.%Y %H:%M:%S UTC"))
            });
        return Some(format!(
            "Permit2 AllowanceExpired: срок разрешения Permit2 → Universal Router истёк{when} — на состоянии этой \
             пробы нового разрешения (approve) нет"
        ));
    }
    if let Some(arg) = arg_of(InsufficientAllowance::SELECTOR, data, message) {
        let how = arg.map_or(String::new(), |a| format!(" (разрешено {a} ед. токена)"));
        return Some(format!(
            "Permit2 InsufficientAllowance: разрешения Permit2 → Universal Router не хватает на сумму обмена{how}"
        ));
    }
    None
}

/// Есть ли откат с селектором `sel`; Some(аргумент uint256, если он прочитан). Данные отката — первыми, затем текст
/// узла (hex после селектора). Ошибок нет.
fn arg_of(sel: [u8; 4], data: &[u8], message: &str) -> Option<Option<U256>> {
    if data.starts_with(&sel) {
        return Some(data.get(4..36).map(U256::from_be_slice));
    }
    let text = message.to_ascii_lowercase();
    let at = text.find(&hex::encode(sel))? + 8;
    let word = text.get(at..at + 64).and_then(|w| hex::decode(w).ok());
    Some(word.map(|w| U256::from_be_slice(&w)))
}
