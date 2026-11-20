//! Файл: venues/bsc_dex/pools/lb20_head.rs
//! Что делает: решение 445 — вызовы и разбор чтения пула Liquidity Book v2.0 на голове (правила — lb_head.rs, ревью 332):
//! состояние (load_lb20::calls — getReservesAndId, feeParameters) и getBin(активный бин памяти) одной пачкой.
//! Кто вызывает: pools/lb_head.rs (calls_for, per_pool, decode — ветка v20).
//! От чего зависит: abi_lb20.rs, load_lb20.rs.
//! Настройки: нет.
//!
//! Источник: lfj-gg/joe-v2, тег v2.0.0 (07f10212) — src/LBPair.sol L147-L158 getReservesAndId, L249-L251 feeParameters,
//! L265-L267 getBin (uint256; резерв бина — uint112, L1048-L1062).

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes};

use crate::core::types::LbBinData;

use super::abi_lb20::ILBPairV20 as P;
use super::load_lb20;

/// Вызовов на пул в пачке головы: состояние (load_lb20::calls) и getBin(активный бин памяти).
pub const PER_POOL: usize = load_lb20::PER_POOL + 1;

/// Вызовы пула на голове: getReservesAndId, feeParameters, getBin(`active`). Принимает: адрес пула, активный бин
/// памяти. Ошибок нет.
pub fn calls(pool: Address, active: u32) -> Vec<(Address, Bytes)> {
    let mut out = load_lb20::calls(pool);
    let c = P::getBinCall {
        id: U24::from(active),
    };
    out.push((pool, Bytes::from(c.abi_encode())));
    out
}

/// Разбор ответов calls (без сети; её же зовут тесты на записанных ответах). Принимает: ответы по порядку, состояние
/// памяти (порядок X/Y), время блока чтения. Возвращает: (состояние на блоке чтения, резервы бина (X, Y)). Ошибка —
/// ответов не PER_POOL, ответ не по ABI, резерв больше u128.
pub fn decode(a: &[Bytes], mem: &LbBinData, ts: u64) -> Result<(LbBinData, (u128, u128)), String> {
    if a.len() != PER_POOL {
        return Err(format!("ответов {} из {PER_POOL}", a.len()));
    }
    let d = load_lb20::decode(&a[..load_lb20::PER_POOL], mem.x_is_token0, ts)?;
    let b = P::getBinCall::abi_decode_returns(&a[load_lb20::PER_POOL])
        .map_err(|e| format!("getBin: {e}"))?;
    let x = u128::try_from(b.reserveX).map_err(|_| "getBin: резерв X больше u128".to_string())?;
    let y = u128::try_from(b.reserveY).map_err(|_| "getBin: резерв Y больше u128".to_string())?;
    Ok((d, (x, y)))
}
