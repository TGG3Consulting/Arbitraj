//! Файл: venues/bsc_dex/quote/v2_pair_sim.rs
//! Что делает: решение 280 — разбор ответов eth_simulateV1 сверки пары V2-форка без роутера через код исполнителя
//! (вызовы собирает quote/v2_pair_quote.rs calls: 0 getReserves → 1 перевод входа из пары → 2 sync → 3 getReserves →
//! 4 approve → 5 execute шагом V2 с нашей комиссией). Чистая функция — проверяется тестом на записанных симуляциях
//! (tests/data/v2_fork_executor_2026-11-15).
//! Кто вызывает: quote/v2_pair_quote.rs (out_at), тесты.
//! От чего зависит: check/simulate.rs (CallResult), math/v2.rs, pools/abi_v2v3.rs.
//! Настройки: нет.

use alloy::sol_types::SolCall;
use alloy_primitives::U256;

use super::super::check::simulate::CallResult;
use super::super::math::v2;
use super::super::pools::abi_v2v3::IV2Pair;

/// Итог сверки по ответам eth_simulateV1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sim {
    /// Пара приняла обмен: выход сети (формула по резервам блока с принятой комиссией), получено владельцем (после
    /// налога выхода, если он есть), формула по резервам после sync (то, что пара отдала при выходе без налога).
    Ok {
        remote: U256,
        received: U256,
        at_sync: U256,
    },
    /// Подготовка не прошла (перевод из пары, sync, approve, ответ getReserves) — сверка не выполнена, причина.
    Prep(String),
    /// Пара откатила обмен шагом V2 с нашей комиссией — причина узла.
    Rejected(String),
}

/// Разбор ответов (чистая функция — тест на записанной симуляции). Принимает: шесть итогов, вход — token0 пары ли,
/// сумму, дробь комиссии. Ошибок нет.
pub fn judge(r: &[CallResult], zero_in: bool, amount: U256, fee: (U256, U256)) -> Sim {
    if r.len() != 6 {
        return Sim::Prep(format!("итогов симуляции {} вместо 6", r.len()));
    }
    let res = |c: &CallResult| -> Option<(U256, U256)> {
        let x = IV2Pair::getReservesCall::abi_decode_returns(&c.return_data).ok()?;
        let (a, b) = (U256::from(x.reserve0), U256::from(x.reserve1));
        Some(if zero_in { (a, b) } else { (b, a) })
    };
    let names = [
        "getReserves",
        "перевод входа из пары",
        "sync()",
        "getReserves",
        "approve",
    ];
    if let Some((i, c)) = r[..5].iter().enumerate().find(|(_, c)| !c.ok()) {
        return Sim::Prep(format!("шаг {i} ({}) откатился: {}", names[i], c.why()));
    }
    if !r[5].ok() {
        return Sim::Rejected(r[5].why());
    }
    let (Some((ri, ro)), Some((si, so))) = (res(&r[0]), res(&r[3])) else {
        return Sim::Prep("ответ getReserves не по ABI — пара нестандартная".into());
    };
    let received = U256::try_from_be_slice(&r[5].return_data).unwrap_or_default();
    match (
        v2::amount_out(amount, ri, ro, fee),
        v2::amount_out(amount, si, so, fee),
    ) {
        (Some(remote), Some(at_sync)) => Sim::Ok {
            remote,
            received,
            at_sync,
        },
        _ => Sim::Prep("резервы пары нулевые".into()),
    }
}
