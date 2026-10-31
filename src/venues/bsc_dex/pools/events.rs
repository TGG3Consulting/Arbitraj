//! Файл: venues/bsc_dex/pools/events.rs
//! Что делает: общий итог применения события пула и выбор разбора по виду пула; topic0 событий для подписки
//! (все события — глубоким пулам, только добавление ликвидности — мелким).
//! Кто вызывает: pools/listen_log.rs.
//! От чего зависит: events_v2.rs, events_v3.rs, events_cl.rs, events_bin.rs, families.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::families::{Family, Proto};
use super::registry::Entry;
use super::{
    events_algebra, events_bin, events_cl, events_dodo, events_elastic, events_solidly, events_v2,
    events_v3,
};

/// Что произошло с записью после события.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// Состояние обновлено — отправить пул ядру.
    Updated,
    /// Событие не меняет нужное нам состояние (например Donate у CL).
    Ignored,
    /// Нужно перечитать резервы этих бинов (Bin) — затем отправить пул ядру.
    Bins(Vec<u32>),
    /// Событие не сходится с картой или не разобрано — пул недостоверен, перечитать целиком.
    Refresh(String),
}

/// topic0 событий, которые слушаются у пулов этого вида. Ошибок нет.
pub fn topics(f: &Family) -> Vec<B256> {
    match f.proto {
        Proto::V2 => events_v2::topics(),
        Proto::PancakeV3 | Proto::UniswapV3 => events_v3::topics(f.proto),
        Proto::InfinityCl | Proto::UniswapV4 => events_cl::topics(f.proto),
        Proto::InfinityBin => events_bin::topics(),
        Proto::KyberElastic => events_elastic::topics(),
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => events_dodo::topics(),
        Proto::Algebra => events_algebra::topics(),
        Proto::Solidly => events_solidly::topics(),
    }
}

/// topic0 событий добавления ликвидности — у пулов мельче dex.min_pool_liquidity_usd слушаются только они
/// (pools/depth.rs): V2 Mint (IUniswapV2Pair.sol, v2-core 6a9e7c97), V3 Mint (IPancakeV3PoolEvents.sol#L21-L62),
/// CL ModifyLiquidity (ICLPoolManager.sol, Uniswap IPoolManager.sol — bsc_v4.md, раздел 4), Bin Mint
/// (IBinPoolManager.sol#L88-L104). Ошибок нет.
pub fn liquidity_topics(f: &Family) -> Vec<B256> {
    use super::abi_v2v3::{IV2Pair, IV3Pool};
    use super::abi_v4::{IInfinityBin, IInfinityCl, IUniswapV4};
    use alloy::sol_types::SolEvent;
    match f.proto {
        Proto::V2 => vec![IV2Pair::Mint::SIGNATURE_HASH],
        Proto::PancakeV3 | Proto::UniswapV3 => vec![IV3Pool::Mint::SIGNATURE_HASH],
        Proto::InfinityCl => vec![IInfinityCl::ModifyLiquidity::SIGNATURE_HASH],
        Proto::UniswapV4 => vec![IUniswapV4::ModifyLiquidity::SIGNATURE_HASH],
        Proto::InfinityBin => vec![IInfinityBin::Mint::SIGNATURE_HASH],
        Proto::KyberElastic => events_elastic::liquidity_topics(),
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => events_dodo::liquidity_topics(),
        Proto::Algebra => events_algebra::liquidity_topics(),
        Proto::Solidly => events_solidly::liquidity_topics(),
    }
}

/// Применяет событие к записи. Принимает: запись, лог. Возвращает: Applied. Ошибок нет (разбор не удался → Refresh).
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let r = match e.family.proto {
        Proto::V2 => events_v2::apply(e, log),
        Proto::PancakeV3 | Proto::UniswapV3 => events_v3::apply(e, log),
        Proto::InfinityCl | Proto::UniswapV4 => events_cl::apply(e, log),
        Proto::InfinityBin => events_bin::apply(e, log),
        Proto::KyberElastic => events_elastic::apply(e, log),
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => events_dodo::apply(e, log),
        Proto::Algebra => events_algebra::apply(e, log),
        Proto::Solidly => events_solidly::apply(e, log),
    };
    if let Some(b) = log.block_number
        && matches!(r, Applied::Updated | Applied::Bins(_))
    {
        e.pool.block = b;
        // Позиция последнего применённого события: остальные события того же блока (logIndex больше) ещё впереди.
        e.synced_block = b;
        e.synced_log = log.log_index;
        e.pool.received_at = chrono::Utc::now();
    }
    r
}
