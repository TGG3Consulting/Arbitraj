//! Файл: venues/bsc_dex/check/service_tax_calls.rs
//! Что делает: вызовы пробы переводов USDT для eth_simulateV1 и разбор их итогов (решение 67): налоги из пула, в пул,
//! на пустой адрес и газ перевода (на пустой адрес — верхняя оценка: запись в пустую ячейку баланса дороже).
//! Кто вызывает: check/service_tax.rs; tests/bsc_dex_service_tax.rs (список вызова для записи и живой ответ узла).
//! От чего зависит: check/probe_calls.rs, check/simulate.rs, check/taxes_util.rs (pct).
//! Настройки: нет.
//!
//! ERC-20 transfer / balanceOf — https://eips.ethereum.org/EIPS/eip-20 .

use alloy_primitives::{Address, U256, keccak256};

use crate::core::types::TokenTaxes;

use super::probe_calls as pc;
use super::simulate::{Call, CallResult};
use super::taxes_util::pct;

/// Вызовы пробы переводов USDT для eth_simulateV1 (по порядку): баланс кошелька, перевод суммы из пула-источника на
/// кошелёк, баланс кошелька; баланс пула, перевод половины в пул, баланс пула; баланс пустого адреса (последние 20 байт
/// keccak256(кошелёк) — вычисляется, не зашит), перевод четверти на него, его баланс. Ошибок нет.
pub fn calls(usdt: Address, source: Address, wallet: Address, amount: U256) -> Vec<Call> {
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    let (half, quarter) = (amount / U256::from(2u8), amount / U256::from(4u8));
    vec![
        pc::balance(wallet, usdt, wallet),
        pc::transfer(source, usdt, wallet, amount),
        pc::balance(wallet, usdt, wallet),
        pc::balance(wallet, usdt, source),
        pc::transfer(wallet, usdt, source, half),
        pc::balance(wallet, usdt, source),
        pc::balance(wallet, usdt, empty),
        pc::transfer(wallet, usdt, empty, quarter),
        pc::balance(wallet, usdt, empty),
    ]
}

/// Итоги пробы (вызовы — `calls`). Принимает: итоги, сумму пробы, знаки USDT. Возвращает: налоги (из пула, в пул, на
/// адрес) и газ перевода на адрес. Ошибки (текст): шаг откатился, ответ баланса не по ABI, сумма пробы 0.
pub fn read(r: &[CallResult], amount: U256, dec: u8) -> Result<(TokenTaxes, u64), String> {
    if let Some((i, c)) = r.iter().enumerate().find(|(_, c)| !c.ok()) {
        return Err(format!("шаг {i} пробы перевода откатился: {}", c.why()));
    }
    let b = |i: usize| r.get(i).and_then(pc::u256);
    let (Some(w0), Some(w1), Some(s0), Some(s1), Some(e0), Some(e1)) =
        (b(0), b(2), b(3), b(5), b(6), b(8))
    else {
        return Err("ответ баланса не по ABI".into());
    };
    let (half, quarter) = (amount / U256::from(2u8), amount / U256::from(4u8));
    let got = |a: U256, b: U256| b.saturating_sub(a);
    let taxes = TokenTaxes {
        buy_pct: pct(amount, got(w0, w1), dec).ok_or("сумма пробы 0 — налог не определить")?,
        sell_pct: pct(half, got(s0, s1), dec).ok_or("сумма пробы 0 — налог не определить")?,
        transfer_pct: pct(quarter, got(e0, e1), dec)
            .ok_or("сумма пробы 0 — налог не определить")?,
    };
    Ok((taxes, r[7].gas_used.to()))
}
