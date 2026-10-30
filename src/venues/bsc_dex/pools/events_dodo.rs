//! Файл: venues/bsc_dex/pools/events_dodo.rs
//! Что делает: живое состояние пула DODO PMM (DVM/DPP/DSP, решения 80, 86b) по событиям. Любое событие,
//! меняющее кривую (DODOSwap, BuyShares, SellShares, RChange, DODOFlashLoan), ведёт к Refresh — пул
//! перечитывается целиком (load_dodo.rs). Причина (решение 23): ни одно событие DODO не несёт пост-состояние
//! кривой (DODOSwap — только суммы обмена; RChange — только новое R; события долей — только shares), а выход PMM
//! зависит от i, K, B, Q, B0, Q0, R — точечно по событию их достоверно не восстановить. Поэтому «событийное зрение»
//! для PMM = метка «пул изменился → перечитать» (целевой Multicall3 всё равно идёт перед сделкой, решение 83).
//! Пошаговое обновление резервов из DODOSwap без перечитывания — кандидат на следующую фазу (нужна сверка на
//! реально записанных ответах узла).
//! Кто вызывает: pools/events.rs.
//! От чего зависит: pools/abi_dodo.rs.
//! Настройки: нет.
//!
//! События и поля — DODOEX/contractV2 (решение 23): DODOSwap/DODOFlashLoan (DVMTrader.sol), BuyShares/SellShares
//! (DVMFunding.sol/DSPFunding.sol), RChange (DSPTrader.sol/DPPTrader.sol). DPP-пулы долей не выпускают (частный
//! пул), но подписка на лишний topic0 безвредна — пул всё равно перечитывается по DODOSwap/RChange.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use super::abi_dodo::IDodoPool;
use super::events::Applied;
use super::registry::Entry;

/// topic0 всех событий пула DODO (все ведут к перечитыванию). Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        IDodoPool::DODOSwap::SIGNATURE_HASH,
        IDodoPool::BuyShares::SIGNATURE_HASH,
        IDodoPool::SellShares::SIGNATURE_HASH,
        IDodoPool::RChange::SIGNATURE_HASH,
        IDodoPool::DODOFlashLoan::SIGNATURE_HASH,
    ]
}

/// topic0 добавления ликвидности — мелкие пулы (pools/depth.rs) слушают только его: у DVM/DSP ликвидность
/// добавляется долями (BuyShares). У DPP долей нет — мелкий DPP-пул пробуждается общей перепроверкой (решение 73).
/// Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![IDodoPool::BuyShares::SIGNATURE_HASH]
}

/// Применяет событие DODO: любое известное событие кривой → Refresh (перечитать пул, см. шапку). Ошибок нет.
pub fn apply(_e: &mut Entry, log: &Log) -> Applied {
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    if t0 == IDodoPool::DODOSwap::SIGNATURE_HASH {
        return Applied::Refresh("DODO DODOSwap: перечитать кривую (i/K/B/Q/цели/R)".into());
    }
    if t0 == IDodoPool::BuyShares::SIGNATURE_HASH || t0 == IDodoPool::SellShares::SIGNATURE_HASH {
        return Applied::Refresh(
            "DODO доли (BuyShares/SellShares): перечитать резервы и цели".into(),
        );
    }
    if t0 == IDodoPool::RChange::SIGNATURE_HASH {
        return Applied::Refresh("DODO RChange: перечитать кривую".into());
    }
    if t0 == IDodoPool::DODOFlashLoan::SIGNATURE_HASH {
        return Applied::Refresh("DODO DODOFlashLoan: перечитать резервы".into());
    }
    Applied::Ignored
}
