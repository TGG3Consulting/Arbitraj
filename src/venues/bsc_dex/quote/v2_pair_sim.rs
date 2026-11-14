//! Файл: venues/bsc_dex/quote/v2_pair_sim.rs
//! Что делает: решение 280 — разбор ответов eth_simulateV1 сверки пары V2-форка без роутера через код исполнителя
//! (вызовы — quote/v2_pair_calls.rs build: 0 getReserves → 1 перевод входа из пары исполнителю → 2 sync →
//! 3 getReserves → 4 balanceOf исполнителя → 5 execute (amountIn = 0, весь баланс) → 6 getReserves). Доработка по ревью:
//! причины отказа различаются (пара отклонила обмен «K» / сверка не собрана / прочий откат), и пара, отдавшая меньше
//! формулы, исключается: дошло до пары = резерв входа шага 6 − шага 3, отдала = резерв выхода шага 3 − шага 6 (swap
//! пары V2 пишет резервы = балансы); «отдала» сравнивается с формулой V2 по резервам после sync на «дошло» с нашей
//! комиссией в пределах dex.quoter_max_diff_pct. Налог монеты сюда не входит по построению: «дошло» уже после налога
//! перевода входа, «отдала» — до налога перевода выхода (его меряет проба налогов, check/probe.rs).
//! Чистые функции — проверяются тестом на записанных симуляциях (tests/data/v2_executor_balance_2026-11-15).
//! Кто вызывает: quote/v2_pair_quote.rs (out_at), quote/v2_pair_probe.rs (coin_transfer), тесты.
//! От чего зависит: check/simulate.rs (CallResult), math/v2.rs, pools/abi_v2v3.rs, quote/v2_pair_calls.rs (CALLS).
//! Настройки: нет (допуск — параметром).

use alloy::sol_types::{Revert, SolCall, SolError};
use alloy_primitives::U256;
use rust_decimal::Decimal;

use super::super::check::simulate::CallResult;
use super::super::math::v2;
use super::super::pools::abi_v2v3::IV2Pair;
use super::v2_pair_calls::CALLS;

/// Итог сверки по ответам eth_simulateV1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sim {
    /// Пара приняла обмен: выход сети (формула по резервам блока с принятой комиссией на запрошенную сумму), пришло
    /// исполнителю из пары, дошло до пары, отдала пара, формула по резервам после sync на «дошло», получено владельцем.
    Ok {
        remote: U256,
        got: U256,
        arrived: U256,
        paid: U256,
        expected: U256,
        received: U256,
    },
    /// Сверка не собрана (перевод из пары не прошёл или обложен целиком, sync, ответ getReserves / balanceOf).
    Prep(String),
    /// Пара отклонила обмен проверкой K — комиссия пары выше нашей или обмен нестандартный (причина узла).
    Rejected(String),
    /// Обмен откатился по другой причине (перевод монеты, требования исполнителя…) — причина узла.
    Failed(String),
    /// Пара отдала меньше формулы больше допуска — причина с цифрами.
    Underpaid(String),
}

/// Текст отката Error(string) (Solidity require). None — не Error(string). Ошибок нет.
pub fn reason(r: &CallResult) -> Option<String> {
    Revert::abi_decode(&r.return_data).ok().map(|e| e.reason)
}

/// Откат проверки K пары V2 («UniswapV2: K», у форков «<Имя>: K» — tests/data/v2_fork_executor_2026-11-15). Ошибок нет.
pub fn pair_k(r: &CallResult) -> bool {
    reason(r).is_some_and(|w| w == "K" || w.ends_with(": K"))
}

/// Откат перевода монеты из пары («…: TRANSFER_FAILED» _safeTransfer пары V2 — механика монеты: запрет торговли,
/// антибот). Ошибок нет.
pub fn coin_transfer(r: &CallResult) -> bool {
    reason(r).is_some_and(|w| w.ends_with("TRANSFER_FAILED"))
}

/// Разбор ответов (см. шапку). Принимает: семь итогов, вход — token0 пары ли, сумму, дробь комиссии, допуск, %.
/// Ошибок нет.
pub fn judge(
    r: &[CallResult],
    zero_in: bool,
    amount: U256,
    fee: (U256, U256),
    limit: Decimal,
) -> Sim {
    if r.len() != CALLS {
        return Sim::Prep(format!("итогов симуляции {} вместо {CALLS}", r.len()));
    }
    let res = |c: &CallResult| -> Option<(U256, U256)> {
        let x = IV2Pair::getReservesCall::abi_decode_returns(&c.return_data).ok()?;
        let (a, b) = (U256::from(x.reserve0), U256::from(x.reserve1));
        Some(if zero_in { (a, b) } else { (b, a) })
    };
    let names = [
        "getReserves",
        "перевод из пары",
        "sync()",
        "getReserves",
        "balanceOf",
    ];
    if let Some((i, c)) = r[..5].iter().enumerate().find(|(_, c)| !c.ok()) {
        return Sim::Prep(format!("шаг {i} ({}) откатился: {}", names[i], c.why()));
    }
    let got = U256::try_from_be_slice(&r[4].return_data).unwrap_or_default();
    if got.is_zero() {
        return Sim::Prep(format!(
            "перевод из пары обложен целиком: исполнителю пришло 0 из {amount}"
        ));
    }
    if !r[5].ok() {
        return match pair_k(&r[5]) {
            true => Sim::Rejected(r[5].why()),
            false => Sim::Failed(r[5].why()),
        };
    }
    let (Some((ri, ro)), Some((si, so)), Some((ai, ao))) = (res(&r[0]), res(&r[3]), res(&r[6]))
    else {
        return Sim::Prep("ответ getReserves не по ABI — пара нестандартная".into());
    };
    let (Some(arrived), Some(paid)) = (ai.checked_sub(si), so.checked_sub(ao)) else {
        return Sim::Underpaid(
            "резервы пары после обмена не сходятся с обменом (вход убыл или выход вырос)".into(),
        );
    };
    let received = U256::try_from_be_slice(&r[5].return_data).unwrap_or_default();
    let (Some(remote), Some(expected)) = (
        v2::amount_out(amount, ri, ro, fee),
        v2::amount_out(arrived, si, so, fee),
    ) else {
        return Sim::Prep("резервы пары нулевые".into());
    };
    // Допуск в процентах → сравнение в целых: paid · 100·10^s < expected · (100 − limit)·10^s.
    let scale = Decimal::from(100) - limit;
    let (m, s) = (scale.mantissa().max(0) as u128, scale.scale());
    let hundred = U256::from(100u8) * U256::from(10u8).pow(U256::from(s));
    if paid.saturating_mul(hundred) < expected.saturating_mul(U256::from(m)) {
        return Sim::Underpaid(format!(
            "пара отдала {paid} при формуле V2 {expected} на дошедшие {arrived} (допуск {limit} %)"
        ));
    }
    Sim::Ok {
        remote,
        got,
        arrived,
        paid,
        expected,
        received,
    }
}
