//! Файл: tests/arb_dex_algebra_dir_events.rs
//! Что проверяет: решение 444 (T433-F5) — события пула вида algebra_dir_fee (Camelot V3) на записи своего узла
//! Arbitrum (logs.json — eth_getLogs пулов за [B − --log-blocks, B]; pool_<адрес>.json — состояние на блоке B):
//! 1. Каждый лог Fee разбирается ABI ветки direction-based-fee (IAlgebraPoolEvents.sol:107) и применяется
//!    events_algebra_dir::apply (Updated, обе комиссии); после всех Fee пула комиссии записи = feeZto / feeOtz
//!    globalState на блоке B (комиссии меняет только _updateFee, и он всегда шлёт Fee — AlgebraPool.sol:549-558).
//! 2. Последний лог пула — Swap: после применения цена, тик и ликвидность = globalState и liquidity() на блоке B.
//! 3. За каждым Fee в той же транзакции идёт ещё лог пула (Swap / Mint / Burn) — основание Follow в pools/settle.rs.
//! Тесты с меткой ignore до записи (метку снять после записи — docs/TASKS.md, «Решение 444»).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/algebra_dir_common, tests/native_common (entry), arb::venues::bsc_dex::pools::{events,
//! abi_algebra_dir, abi_algebra}.

mod algebra_dir_common;
mod native_common;

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;

use arb::core::types::{PoolRef, PoolState};
use arb::venues::bsc_dex::pools::abi_algebra::IAlgebraPool;
use arb::venues::bsc_dex::pools::abi_algebra_dir::IAlgebraDirPool;
use arb::venues::bsc_dex::pools::events::{self, Applied};

use algebra_dir_common::{global_state, json, pool_state, pools};

/// Логи записи по пулам в порядке цепи (блок, номер лога). Ошибок нет (тест падает).
fn logs() -> Vec<Log> {
    let l = json("logs.json");
    let list = l["response"]["result"].as_array().expect("eth_getLogs без ответа");
    let mut v: Vec<Log> = list
        .iter()
        .map(|j| serde_json::from_value(j.clone()).unwrap())
        .collect();
    v.sort_by_key(|g| (g.block_number, g.log_index));
    v
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/algebra_dir_fee.py"]
fn fee_and_swap_logs_rebuild_recorded_state() {
    let all = logs();
    for p in pools() {
        let addr: alloy_primitives::Address = p["pool"].as_str().unwrap().parse().unwrap();
        let (state, _) = pool_state(&p);
        let gs = global_state(&p);
        let mut e = native_common::entry("algebra_dir_fee", None);
        e.pool.reference = PoolRef::Contract(addr);
        e.pool.state = PoolState::Algebra {
            sqrt_price_x96: alloy_primitives::U256::ZERO,
            tick: 0,
            liquidity: 0,
            fee: 0,
            fee_otz: None,
        };
        let mine: Vec<&Log> = all.iter().filter(|g| g.address() == addr).collect();
        let mut fees = 0;
        for (i, g) in mine.iter().enumerate() {
            let t0 = g.topic0().copied();
            let applied = events::apply(&mut e, g);
            if t0 == Some(IAlgebraDirPool::Fee::SIGNATURE_HASH) {
                assert_eq!(applied, Applied::Updated, "{addr}: Fee применяется сразу");
                fees += 1;
                let next = mine.get(i + 1).expect("за Fee нет лога пула в записи");
                assert_eq!(
                    next.transaction_hash, g.transaction_hash,
                    "{addr}: Fee — не последний лог tx"
                );
            }
        }
        let PoolState::Algebra { fee, fee_otz, .. } = &e.pool.state else {
            panic!("не Algebra");
        };
        if fees > 0 {
            assert_eq!(*fee, u32::from(gs.feeZto), "{addr}: feeZto");
            assert_eq!(*fee_otz, Some(u32::from(gs.feeOtz)), "{addr}: feeOtz");
        }
        let last_swap = mine
            .last()
            .is_some_and(|g| g.topic0() == Some(&IAlgebraPool::Swap::SIGNATURE_HASH));
        if last_swap {
            let PoolState::Algebra {
                sqrt_price_x96,
                tick,
                liquidity,
                ..
            } = state
            else {
                panic!("не Algebra");
            };
            let PoolState::Algebra {
                sqrt_price_x96: s,
                tick: t,
                liquidity: l,
                ..
            } = e.pool.state
            else {
                panic!("не Algebra");
            };
            assert_eq!((s, t, l), (sqrt_price_x96, tick, liquidity), "{addr}: после Swap");
        }
    }
}
