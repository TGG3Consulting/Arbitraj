//! Файл: venues/bsc_dex/pools/created_log.rs
//! Что делает: одно событие создания пула → пул подписки (решение 58: между обновлениями справочника). Источник события
//! (адрес контракта и topic0) сверяется с подключёнными DEX: V2 PairCreated / V3 PoolCreated своей фабрики — пул из
//! события проверяется на узле (verify.rs: токены, реестр фабрики, своя комиссия пары) и подключается; V4 Initialize
//! своего менеджера — ключ из события, PoolId сверяется (discover_v4::parse), затем та же проверка ключа у менеджера.
//! Пул из события попадает в справочник (показ и итог проверки — index::record). Встречный токен — наблюдателя.
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
use super::super::directory::gt_parse::IndexedPool;
use super::super::directory::pool_key;
use super::abi_dodo::IDodoFactory;
use super::abi_elastic::IElasticFactory;
use super::abi_v2v3::{IV2Factory, IV3Factory};
use super::abi_v4::{IInfinityBin, IInfinityCl, IUniswapV4};
use super::attach;
use super::created::Watch;
use super::discover::against;
use super::discover_v4;
use super::families::{self, Family, Proto};
use super::index;
use super::listen::Pools;
use super::verify::{self, Candidate, Verdict};

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
            // KyberSwap Elastic (решение 81): PoolCreated своей фабрики — пул отдельным контрактом (как V3).
            Proto::KyberElastic => (
                "dex.factory",
                IElasticFactory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            // DODO PMM (решение 86b): своё событие создания у фабрики DVM/DPP/DSP (пул — отдельный контракт).
            Proto::DodoDvm => ("dex.factory", IDodoFactory::NewDVM::SIGNATURE_HASH, false),
            Proto::DodoDpp => ("dex.factory", IDodoFactory::NewDPP::SIGNATURE_HASH, false),
            Proto::DodoDsp => ("dex.factory", IDodoFactory::NewDSP::SIGNATURE_HASH, false),
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
    let c = if v4 {
        let all = discover_v4::parse(&p.ctx, f, &log).map_err(|e| e.message)?;
        let Some(x) = against(&all, w.usdt) else {
            return Ok(());
        };
        let pool = pool_key(&x.reference);
        Candidate {
            family: f,
            pool,
            coin: x.coin,
            quote: w.usdt,
        }
    } else {
        candidate(f, &log, w.usdt)?
    };
    if !w.coins.contains_key(&c.coin) {
        return Ok(());
    }
    // Пул из события — тоже в справочнике (источник — событие, не индексатор) и в той же проверке (index::record).
    remember(p, w, &c);
    let v = verify::verify(&p.ctx, std::slice::from_ref(&c))
        .await
        .map_err(|e| e.message)?;
    index::record(&p.ctx, std::slice::from_ref(&c), v.clone()).await;
    let found = match v.into_iter().next() {
        Some(Verdict::Taken(found)) => vec![*found],
        Some(Verdict::Rejected(why)) => {
            return Err(format!("{} {}: отклонён — {why}", f.id, c.pool));
        }
        Some(Verdict::Failed(why)) => {
            return Err(format!("{} {}: не проверен (узел) — {why}", f.id, c.pool));
        }
        None => Vec::new(),
    };
    attach::attach(p, w, found, true)
        .await
        .map(|_| ())
        .map_err(|e| e.message)
}

/// Пул из события — в справочник (поле events: показ на «Монетах», перепроверка вместе с пулами индексатора).
/// Ошибок нет.
fn remember(p: &Pools, w: &Watch, c: &Candidate) {
    let sym = |a: Address| {
        w.coins
            .get(&a)
            .cloned()
            .unwrap_or_else(|| format!("{a:#x}"))
    };
    let item = IndexedPool {
        dex_id: c.family.id.to_string(),
        pool: c.pool.clone(),
        base: c.coin,
        quote: c.quote,
        base_symbol: sym(c.coin),
        quote_symbol: sym(c.quote),
        reserve_usd: None,
    };
    p.ctx.index.with(|s| {
        let list = s.events.entry(c.coin).or_default();
        if !list.iter().any(|x| x.pool == item.pool) {
            list.push(item);
        }
    });
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
        // KyberSwap Elastic (решение 81): своё событие PoolCreated (swapFeeUnits/tickDistance вместо fee).
        Proto::KyberElastic => {
            let e = IElasticFactory::PoolCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PoolCreated (Elastic) не по ABI: {e}"))?;
            (e.token0, e.token1, e.pool)
        }
        // DODO PMM (решение 86b): NewDVM/NewDPP/NewDSP(baseToken, quoteToken, creator, pool) — поля одинаковы.
        Proto::DodoDvm => {
            let e = IDodoFactory::NewDVM::decode_log_data(&log.inner.data)
                .map_err(|e| format!("NewDVM не по ABI: {e}"))?;
            (e.baseToken, e.quoteToken, e.dvm)
        }
        Proto::DodoDpp => {
            let e = IDodoFactory::NewDPP::decode_log_data(&log.inner.data)
                .map_err(|e| format!("NewDPP не по ABI: {e}"))?;
            (e.baseToken, e.quoteToken, e.dpp)
        }
        Proto::DodoDsp => {
            let e = IDodoFactory::NewDSP::decode_log_data(&log.inner.data)
                .map_err(|e| format!("NewDSP не по ABI: {e}"))?;
            (e.baseToken, e.quoteToken, e.DSP)
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
