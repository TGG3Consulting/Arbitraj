//! Файл: venues/bsc_dex/pools/created_log.rs
//! Что делает: одно событие создания пула → пул подписки (решение 58: между обновлениями справочника). Источник события
//! (адрес контракта и topic0) сверяется с подключёнными DEX: V2 PairCreated / V3 PoolCreated своей фабрики — пул из
//! события проверяется на узле (verify.rs: токены, реестр фабрики, своя комиссия пары) и подключается; V4 Initialize
//! своего менеджера — ключ из события, PoolId сверяется (discover_v4::parse). Встречный токен — наблюдателя.
//! Кто вызывает: pools/created.rs.
//! От чего зависит: ctx.rs, families.rs, verify.rs, discover_v4.rs, discover.rs (against), attach.rs, abi_v2v3.rs, abi_v4.rs.
//! Настройки: dex.factory, dex.pool_manager [DEX].
//!
//! События — research/flow/bsc_dex.md (схема п. 2: PairCreated(token0, token1, pair, n), PoolCreated(token0, token1,
//! fee, tickSpacing, pool)) и research/flow/bsc_v4.md, раздел 3 (Initialize).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};

use super::super::ctx::Ctx;
use super::abi_v2v3::{IV2Factory, IV3Factory};
use super::abi_v4::{IInfinityBin, IInfinityCl, IUniswapV4};
use super::attach;
use super::created::Watch;
use super::discover::against;
use super::discover_v4;
use super::families::{self, Family, Proto};
use super::listen::Pools;
use super::verify::{self, Candidate};

/// Источники событий создания подключённых DEX: (адрес, topic0, V4 ли, вид). Ошибок нет.
pub fn sources(ctx: &Ctx) -> Vec<(Address, B256, bool, &'static Family)> {
    let mut out = Vec::new();
    for f in families::all(ctx) {
        if families::classify(ctx, f.id).is_err() {
            continue;
        }
        let (setting, t0, v4) = match f.proto {
            Proto::V2 => (
                "dex.factory",
                IV2Factory::PairCreated::SIGNATURE_HASH,
                false,
            ),
            Proto::PancakeV3 | Proto::UniswapV3 => (
                "dex.factory",
                IV3Factory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            Proto::InfinityCl => (
                "dex.pool_manager",
                IInfinityCl::Initialize::SIGNATURE_HASH,
                true,
            ),
            Proto::InfinityBin => (
                "dex.pool_manager",
                IInfinityBin::Initialize::SIGNATURE_HASH,
                true,
            ),
            Proto::UniswapV4 => (
                "dex.pool_manager",
                IUniswapV4::Initialize::SIGNATURE_HASH,
                true,
            ),
        };
        if let Ok(addr) = ctx.dex_addr(setting, f.id) {
            out.push((addr, t0, v4, f));
        }
    }
    out
}

/// Событие → проверенный пул → подписка. Принимает: окружение, наблюдателя, сырой лог.
/// Ошибка — текст (лог не разобран, источник не подключён, проверка не прошла, узел).
pub async fn on_log(p: &Pools, w: &Watch, raw: &str) -> Result<(), String> {
    let log: Log = serde_json::from_str(raw).map_err(|e| format!("лог не разобран: {e}"))?;
    let t0 = log.topics().first().copied().unwrap_or_default();
    let Some((_, _, v4, f)) = sources(&p.ctx)
        .into_iter()
        .find(|(a, t, _, _)| *a == log.inner.address && *t == t0)
    else {
        return Ok(()); // DEX выключили на лету — событие не нужно.
    };
    let found = if v4 {
        let all = discover_v4::parse(&p.ctx, f, &log).map_err(|e| e.message)?;
        against(&all, w.usdt).into_iter().collect()
    } else {
        let c = candidate(f, &log, w.usdt)?;
        let r = verify::verify(&p.ctx, std::slice::from_ref(&c))
            .await
            .map_err(|e| e.message)?;
        match r.into_iter().next() {
            Some(Ok(found)) => vec![found],
            Some(Err(why)) => return Err(format!("{} {}: отклонён — {why}", f.id, c.pool)),
            None => Vec::new(),
        }
    };
    let found = found
        .into_iter()
        .filter(|x| w.coins.contains_key(&x.coin))
        .collect();
    attach::attach(p, w, found, true)
        .await
        .map(|_| ())
        .map_err(|e| e.message)
}

/// Кандидат из события V2/V3: пул, монета (не встречный токен), встречный токен. Ошибка — лог не по ABI или во
/// встречном поле не тот токен.
pub fn candidate(f: &'static Family, log: &Log, quote: Address) -> Result<Candidate, String> {
    let (t0, t1, pool) = match f.proto {
        Proto::V2 => {
            let e = IV2Factory::PairCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PairCreated не по ABI: {e}"))?;
            (e.token0, e.token1, e.pair)
        }
        _ => {
            let e = IV3Factory::PoolCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PoolCreated не по ABI: {e}"))?;
            (e.token0, e.token1, e.pool)
        }
    };
    let coin = if t0 == quote {
        t1
    } else if t1 == quote {
        t0
    } else {
        return Err(format!("в событии нет встречного токена {quote}"));
    };
    Ok(Candidate {
        family: f,
        pool: format!("{pool:#x}"),
        coin,
        quote,
    })
}
