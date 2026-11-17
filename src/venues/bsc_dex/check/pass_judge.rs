//! Файл: venues/bsc_dex/check/pass_judge.rs
//! Что делает: решение 401 (t186) — чистый разбор пробы шага пула менеджера ЧЕРЕЗ ИСПОЛНИТЕЛЬ кошелька (без узла;
//! пробу собирает check/pass_probe.rs): по каждой стороне (покупка монеты, продажа купленного) — итог вызова execute,
//! ответ квотера DEX в той же симуляции перед ним и остаток исполнителя по обоим токенам шага после него. Итог пула:
//! - «прошёл» — обе стороны: execute не откатился, получено > 0 и совпало с квотером в допуске dex.quoter_max_diff_pct,
//!   остаток на исполнителе 0 по обоим токенам;
//! - «отвергнут» — execute откатился крючком пула (WrappedError с адресом крючка ключа — hook_revert), откатился при
//!   ответе квотера (квотер обмен пропускает, исполнитель — нет), получено не совпало с квотером, остаток не 0;
//! - «не проверено» — срок транзакции вышел, квотер не ответил (сверить не с чем), откатились и квотер, и исполнитель
//!   без крючка (пул не принимает сумму пробы — свойство суммы, не исполнителя), ответ не по ABI.
//!
//! WrappedError(address target, bytes4 selector, bytes reason, bytes details) — официальный код (копии в
//! contracts/evmtest/vendor): v4-core src/libraries/CustomRevert.sol:11 и Hooks.sol:137, infinity-core
//! src/libraries/CustomRevert.sol:9 и Hooks.sol:83 — откат крючка менеджер оборачивает bubbleUpAndRevertWith(адрес
//! крючка, селектор функции крючка, HookCallFailed). Запись узла tests/data/executor_v3_live_2026-11-17: откаты крючков
//! 0xddc53a1a… и 0x70201397… — селектор 0x90bfb865, details = HookCallFailed() 0xa9e35b2f (тест
//! tests/bsc_dex_pool_pass_401.rs).
//! Узел BSC отдаёт данные отката шага eth_simulateV1 в тексте error.message («execution reverted: 0x…»), returnData
//! пуст (та же запись) — данные берутся из текста, иначе из returnData.
//! Кто вызывает: check/pass_probe.rs, exec/executor/pool_pass.rs (on_sim), тесты.
//! От чего зависит: check/{simulate, probe_calls}.rs, exec/executor/abi.rs (возврат execute).
//! Настройки: нет (допуск dex.quoter_max_diff_pct передаёт вызывающий).

use alloy::sol_types::{Revert, SolCall, SolError};
use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;

use super::super::exec::executor::abi::ArbExecutor;
use super::probe_calls;
use super::simulate::CallResult;

alloy::sol! {
    error WrappedError(address target, bytes4 selector, bytes reason, bytes details);
}

/// Одна сторона пробы.
#[derive(Debug, Clone)]
pub struct Side<'a> {
    /// «покупка» / «продажа» — для текста.
    pub name: &'static str,
    /// Итог вызова execute.
    pub swap: &'a CallResult,
    /// Ответ квотера DEX в том же состоянии перед вызовом; None — квотер откатился или ответ не по ABI.
    pub quoted: Option<U256>,
    /// balanceOf(исполнитель) по обоим токенам шага после вызова; None — не прочитан.
    pub left: [Option<U256>; 2],
}

/// Итог пробы пула.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Rejected(String),
    Unchecked(String),
}

/// Данные отката шага: hex из текста «execution reverted: 0x…» (узел BSC), иначе returnData. None — нет. Ошибок нет.
pub fn revert_data(r: &CallResult) -> Option<Bytes> {
    let msg = r.error.as_ref().map(|e| e.message.as_str()).unwrap_or("");
    if let Some(i) = msg.find("0x")
        && let Ok(b) = alloy_primitives::hex::decode(msg[i + 2..].trim())
    {
        return Some(Bytes::from(b));
    }
    (!r.return_data.is_empty()).then(|| r.return_data.clone())
}

/// Откат «срок транзакции вышел» — не итог пула: check/probe_calls.rs late (Universal Router, `require(…, "deadline")`
/// исполнителя в returnData) или текст узла с «deadline» (так же, как exec/executor/sim.rs received: узел BSC кладёт
/// причину отката в error.message). Ошибок нет.
pub fn late(r: &CallResult) -> bool {
    let text = r
        .error
        .as_ref()
        .is_some_and(|e| e.message.contains("deadline"));
    !r.ok() && (probe_calls::late(r) || text)
}

/// Откат крючка пула: WrappedError с target = крючок ключа пула (не нулевой). Принимает: итог вызова, крючок. Возвращает:
/// текст «крючок 0x… отверг вызов (функция крючка 0x…): причина» или None — откат не этим крючком. Ошибок нет.
pub fn hook_revert(r: &CallResult, hooks: Address) -> Option<String> {
    if r.ok() || hooks == Address::ZERO {
        return None;
    }
    let w = WrappedError::abi_decode(&revert_data(r)?).ok()?;
    if w.target != hooks {
        return None;
    }
    let reason = match Revert::abi_decode(&w.reason) {
        Ok(e) => e.reason,
        Err(_) => format!("0x{}", alloy_primitives::hex::encode(&w.reason)),
    };
    Some(format!(
        "крючок {hooks:#x} отверг вызов исполнителя (функция крючка {}): {reason}",
        w.selector
    ))
}

/// Текст отката для причины: WrappedError — разобранный (цель, причина), иначе текст узла. Ошибок нет.
pub fn revert_text(r: &CallResult) -> String {
    if let Some(w) = revert_data(r).and_then(|d| WrappedError::abi_decode(&d).ok()) {
        let reason = Revert::abi_decode(&w.reason)
            .map(|e| e.reason)
            .unwrap_or_else(|_| format!("0x{}", alloy_primitives::hex::encode(&w.reason)));
        return format!(
            "WrappedError: контракт {:#x} (функция {}) — {reason}",
            w.target, w.selector
        );
    }
    r.why()
}

/// Разбор стороны. Принимает: сторону, крючок ключа, допуск dex.quoter_max_diff_pct (%). Возвращает: Ok(получено) или
/// Err(итог пула). Ошибок иных нет.
pub fn side(s: &Side, hooks: Address, max_pct: Decimal) -> Result<U256, Outcome> {
    let n = s.name;
    if !s.swap.ok() {
        if late(s.swap) {
            return Err(Outcome::Unchecked(format!(
                "{n}: срок транзакции вышел к симулированному блоку — проверьте dex.deadline_s и свежесть узла"
            )));
        }
        if let Some(w) = hook_revert(s.swap, hooks) {
            return Err(Outcome::Rejected(format!("{n}: {w}")));
        }
        let why = revert_text(s.swap);
        return Err(match s.quoted {
            None => Outcome::Unchecked(format!(
                "{n}: откатились и квотер, и исполнитель — пул не принимает сумму пробы: {why}"
            )),
            Some(q) => Outcome::Rejected(format!(
                "{n}: исполнитель откатился, квотер на ту же сумму ответил {q}: {why}"
            )),
        });
    }
    let Ok(got) = ArbExecutor::executeCall::abi_decode_returns(&s.swap.return_data) else {
        return Err(Outcome::Unchecked(format!(
            "{n}: возврат execute не по ABI"
        )));
    };
    let [Some(a), Some(b)] = s.left else {
        return Err(Outcome::Unchecked(format!(
            "{n}: остаток на исполнителе не прочитан"
        )));
    };
    if !a.is_zero() || !b.is_zero() {
        return Err(Outcome::Rejected(format!(
            "{n}: на исполнителе остались токены шага ({a}, {b}) — должно быть 0"
        )));
    }
    let Some(q) = s.quoted else {
        return Err(Outcome::Unchecked(format!(
            "{n}: квотер DEX не ответил — получено {got} сверить не с чем"
        )));
    };
    if got.is_zero() || q.is_zero() {
        return Err(Outcome::Unchecked(format!(
            "{n}: нулевой выход (получено {got}, квотер {q}) — не доказательство"
        )));
    }
    let diff = if got > q { got - q } else { q - got };
    let pct = ratio_pct(diff, q);
    if pct.is_none_or(|p| p > max_pct) {
        let shown = pct.map_or("?".into(), |p| p.round_dp(6).normalize().to_string());
        return Err(Outcome::Rejected(format!(
            "{n}: получено {got} ≠ квотер {q} ({shown} % > dex.quoter_max_diff_pct {max_pct} %)"
        )));
    }
    Ok(got)
}

/// Итог пула по сторонам. Принимает: покупку, продажу (None — не пробовалась: покупка не прошла), крючок, допуск.
/// Ошибок нет.
pub fn judge(buy: &Side, sell: Option<&Side>, hooks: Address, max_pct: Decimal) -> Outcome {
    if let Err(o) = side(buy, hooks, max_pct) {
        return o;
    }
    let Some(sell) = sell else {
        return Outcome::Unchecked("продажа не пробовалась".into());
    };
    match side(sell, hooks, max_pct) {
        Ok(_) => Outcome::Passed,
        Err(o) => o,
    }
}

/// Расхождение в процентах: diff / base × 100 с 18 знаками после запятой (целочисленно в U256, вниз). None — base = 0
/// или расхождение не помещается в Decimal (больше ~1,7·10²⁰ %) — вызывающий считает это «вне допуска». Ошибок нет.
pub fn ratio_pct(diff: U256, base: U256) -> Option<Decimal> {
    // 10^20 = 100 % × 10^18 (масштаб Decimal 18).
    let scale = U256::from(10u128.pow(20));
    let scaled = diff.checked_mul(scale)?.checked_div(base)?;
    super::super::num::to_decimal(scaled, 18)
}
