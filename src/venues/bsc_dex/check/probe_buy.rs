//! Файл: venues/bsc_dex/check/probe_buy.rs
//! Что делает: хвост первой симуляции пробы налогов (check/probe.rs) — квотер на покупку, баланс монеты кошелька ДО
//! покупки, покупка, баланс ПОСЛЕ — и сколько куплено: прирост баланса (после − до), а не весь баланс.
//! Причина (решения 247/252, дополнение 10.11, Тигран: согласовано; задача t77): перепроверка налогов после отката
//! продажи выхода идёт кошельком сделки (check/recheck.rs coins_by), а на нём лежат монеты сделки (продажа не прошла).
//! Прежде купленным считался весь баланс монеты кошелька после покупки — у кошелька пробы он был равен купленному
//! (монет нет), у кошелька сделки в него вошли бы её монеты: налог покупки занижен, «продажа остатка» и пробный перевод
//! — от чужой суммы. Баланс до покупки читается в той же eth_simulateV1 (тот же блок) — новых обращений к узлу нет.
//! Кто вызывает: check/probe.rs.
//! От чего зависит: check/probe_calls.rs (balance, u256), check/simulate.rs (Call, CallResult).
//! Настроек нет.

use alloy_primitives::{Address, Bytes, U256};

use super::probe_calls as pc;
use super::simulate::{Call, CallResult};

/// Номера вызовов хвоста покупки в списке первой симуляции.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Head {
    /// Квотер на покупку (расчёт без налога).
    pub quote: usize,
    /// Баланс монеты кошелька до покупки.
    pub before: usize,
    /// Сама покупка.
    pub buy: usize,
    /// Баланс монеты кошелька после покупки.
    pub after: usize,
}

/// Дописывает в список (после перевода USDT и разрешений покупки) квотер, баланс до, покупку, баланс после.
/// Принимает: список вызовов, (кошелёк, монета), (адрес и данные квотера), (роутер и данные покупки).
/// Возвращает: номера вызовов. Ошибок нет.
pub fn buy_head(
    list: &mut Vec<Call>,
    (wallet, coin): (Address, Address),
    (qto, qdata): (Address, Bytes),
    (router, data): (Address, Bytes),
) -> Head {
    let quote = list.len();
    list.push(Call {
        from: wallet,
        to: qto,
        input: qdata,
    });
    list.push(pc::balance(wallet, coin, wallet));
    list.push(Call {
        from: wallet,
        to: router,
        input: data,
    });
    list.push(pc::balance(wallet, coin, wallet));
    Head {
        quote,
        before: quote + 1,
        buy: quote + 2,
        after: quote + 3,
    }
}

/// Сколько монет пришло покупкой: баланс после − баланс до. Принимает: итоги balanceOf до и после.
/// Возвращает: None — ответ не по ABI или баланс после меньше баланса до (покупка не дала монет сверх прежнего).
/// Ошибок нет.
pub fn bought(before: &CallResult, after: &CallResult) -> Option<U256> {
    pc::u256(after)?.checked_sub(pc::u256(before)?)
}
