//! Файл: venues/bsc_dex/pools/history_ekubo.rs
//! Что делает: история пулов Ekubo по событиям ядра (решение 454, вопрос 2 решения 447): ключ пула без PoolKeyIndex —
//! из PoolInitialized ядра (как история Initialize менеджера V4, history_v4.rs): ключ события сверяется с PoolId
//! (keccak256), правила ключа — ekubo_accept.rs (расширение, нативный ETH), ключ кладётся в память (ekubo_keys.rs) —
//! дальше проверка и чтение пула идут без реестра. Здесь же — общие для истории V4 и Ekubo развилки: какие виды ведут
//! историю, адрес источника событий, topic0 события создания, разбор лога, PoolId лога для хранения (history_store.rs).
//! Пул с ключом, который правила не берут (расширение не задано и т. п.), в историю пулов не попадает (его событие
//! остаётся в SQLite — после смены настроек разбирается заново при загрузке истории).
//! Кто вызывает: pools/history_v4.rs, history_store.rs, created_log.rs.
//! От чего зависит: ctx.rs, abi_ekubo.rs, ekubo_accept.rs, ekubo_keys.rs, ekubo_route.rs, discover.rs, discover_v4.rs,
//! families.rs.
//! Настройки: dex.ekubo_core [DEX] (источник событий Ekubo); у V4 — dex.pool_manager [DEX]; через ekubo_accept.rs —
//! dex.native_wrapped и адреса расширений.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/interfaces/ICore.sol L111 (PoolInitialized(PoolId, PoolKey,
//! int32 tick, SqrtRatio) — без indexed полей), src/Core.sol L72-L101 (initializePool: событие — при создании пула).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::abi_ekubo::IEkuboCore;
use super::discover::{Found, is_v4};
use super::ekubo_accept::{PoolKeyIn, accept};
use super::families::{Family, Proto};
use super::{discover_v4, ekubo_keys, ekubo_route};

/// Ведёт ли вид историю пулов по событиям создания: V4 / Infinity и Ekubo. Ошибок нет.
pub fn has_history(f: &Family) -> bool {
    is_v4(f) || f.proto == Proto::Ekubo
}

/// Адрес источника событий создания: ядро Ekubo (dex.ekubo_core) или менеджер V4 (dex.pool_manager).
/// Ошибки: NotConfigured.
pub fn source(ctx: &Ctx, f: &Family) -> GatewayResult<Address> {
    let name = if f.proto == Proto::Ekubo {
        "dex.ekubo_core"
    } else {
        "dex.pool_manager"
    };
    ctx.dex_addr(name, f.id)
}

/// topic0 события создания пула вида. Ошибок нет.
pub fn topic(f: &Family) -> B256 {
    if f.proto == Proto::Ekubo {
        IEkuboCore::PoolInitialized::SIGNATURE_HASH
    } else {
        discover_v4::topic(f)
    }
}

/// PoolId лога создания (ключ записи в SQLite): Ekubo — из данных, V4 — topics[1]. Ошибок нет.
pub fn store_id(log: &Log) -> B256 {
    log.topics()
        .get(1)
        .copied()
        .or_else(|| ekubo_route::pool_id(log))
        .unwrap_or_default()
}

/// Разбор события создания вида: Some — пул этого вида, None — не берётся. Ошибки: как у discover_v4::parse_opt;
/// Ekubo — BadResponse (событие не по ABI).
pub fn parse_opt(ctx: &Ctx, f: &'static Family, log: &Log) -> GatewayResult<Option<Found>> {
    if f.proto != Proto::Ekubo {
        return discover_v4::parse_opt(ctx, f, log);
    }
    let e = match IEkuboCore::PoolInitialized::decode_log_data(&log.inner.data) {
        Ok(e) => e,
        Err(x) => {
            let why = format!("{}: PoolInitialized не по ABI: {x}", f.title);
            return Err(ctx.err(ErrorCode::BadResponse, why));
        }
    };
    let k = PoolKeyIn {
        token0: e.poolKey.token0,
        token1: e.poolKey.token1,
        config: e.poolKey.config,
    };
    match accept(ctx, f, log.inner.address, e.poolId, k) {
        Ok((found, key)) => {
            ekubo_keys::remember(ctx.place.id, e.poolId, key);
            Ok(Some(found))
        }
        Err(_) => Ok(None),
    }
}

/// Разбор всех событий создания вида (история). Ошибки: как у parse_opt.
pub fn parse_all(ctx: &Ctx, f: &'static Family, logs: &[Log]) -> GatewayResult<Vec<Found>> {
    if f.proto != Proto::Ekubo {
        return discover_v4::parse_all(ctx, f, logs);
    }
    let mut out = Vec::with_capacity(logs.len());
    for log in logs {
        if let Some(x) = parse_opt(ctx, f, log)? {
            out.push(x);
        }
    }
    Ok(out)
}
