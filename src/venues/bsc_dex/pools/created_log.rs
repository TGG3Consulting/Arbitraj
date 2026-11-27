//! Файл: venues/bsc_dex/pools/created_log.rs
//! Что делает: одно событие создания пула → пул подписки (решение 58: между обновлениями справочника). Источник события
//! (адрес контракта и topic0) сверяется с подключёнными DEX: V2 PairCreated / V3 PoolCreated своей фабрики — пул из
//! события проверяется на узле (verify.rs: токены, реестр фабрики, своя комиссия пары) и подключается; V4 Initialize
//! своего менеджера — ключ из события, PoolId сверяется (discover_v4::parse), затем та же проверка ключа у менеджера
//! (Infinity Stable — тот же Initialize CLPoolManager, пул — если крючок из dex.stable_hooks); решение 453 — события
//! фабрик crypto-пулов Curve (created_crypto.rs).
//! Пул из события попадает в справочник (показ и итог проверки — index::record). Встречный токен — наблюдателя.
//! Кто вызывает: pools/created.rs.
//! От чего зависит: ctx.rs, families.rs, verify.rs, discover_v4.rs, history_ekubo.rs (решение 454: PoolInitialized ядра Ekubo), discover.rs (against), attach.rs, abi_v2v3.rs, abi_v4.rs, ramses.rs (решение 483).
//! Решение 482: Liquidity Book V2.1 — LBPairCreated(tokenX, tokenY, binStep, LBPair, pid) фабрики; Metric OMM —
//! PoolCreated(poolAddress, token0, token1 — indexed, …) фабрики (подписи — исходники Sourcify 42161, abi_lb.rs,
//! abi_metric.rs).
//! Настройки: dex.factory, dex.pool_manager, dex.stable_hooks [DEX].
//!
//! События — research/flow/bsc_dex.md (схема п. 2: PairCreated(token0, token1, pair, n), PoolCreated(token0, token1,
//! fee, tickSpacing, pool)) и research/flow/bsc_v4.md, раздел 3 (Initialize).

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};

use super::super::ctx::Ctx;
use super::super::directory::gt_parse::IndexedPool;
use super::super::directory::pool_key;
use super::abi_aero_basic::IAeroBasicFactory;
use super::abi_algebra::IAlgebraFactory;
use super::abi_basin::IBasinAquifer;
use super::abi_dmm::{self, IDmmFactory};
use super::abi_dodo::IDodoFactory;
use super::abi_ekubo::IEkuboCore;
use super::abi_elastic::IElasticFactory;
use super::abi_lb::ILBFactory;
use super::abi_maverick::IMaverickV2Factory;
use super::abi_metric::IMetricOmmFactory;
use super::abi_slipstream::ICLFactory;
use super::abi_solidly::{ISolidlyFactory, IThenaV1Factory};
use super::abi_v2v3::{IV2Factory, IV3Factory};
use super::abi_v4::{IInfinityBin, IInfinityCl, IUniswapV4};
use super::attach;
use super::created::Watch;
use super::created_crypto;
use super::discover::against;
use super::events_balancer_v3;
use super::families::{self, Family, Proto};
use super::history_ekubo;
use super::index;
use super::indexer_gap;
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
            _ if f.variant == super::family_variant::Variant::SolidlyCl => (
                "dex.factory",
                super::solidly_v3::ISolidlyV3::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            // Nomiswap Stable (решение 284): PairCreated фабрики — та же подпись, что у UniswapV2Factory (L1216).
            // SmarDex (решение 288): PairCreated(token0, token1, pair, uint) — та же подпись (ISmardexFactory.sol L12).
            // Camelot V2 (решение 443): PairCreated(token0 indexed, token1 indexed, pair, uint) — CamelotFactory.sol L23.
            Proto::V2 if f.variant==super::family_variant::Variant::Carbon => ("dex.factory",super::carbon::ICarbon::PairCreated::SIGNATURE_HASH,false),
            Proto::V2 if f.variant == super::family_variant::Variant::MintClub => ("dex.factory",super::mint_club::IBond::TokenCreated::SIGNATURE_HASH,false),
            Proto::V2 if f.variant == super::family_variant::Variant::BunnySwap => continue,
            Proto::V2 | Proto::NomiStable | Proto::Smardex | Proto::CamelotV2 => (
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
            // Infinity Stable (решение «122 (Infinity Stable)»): Initialize того же CLPoolManager; пул берётся, если
            // hooks ключа — из dex.stable_hooks (discover_v4::parse_opt).
            Proto::InfinityStable => (
                "dex.pool_manager",
                IInfinityCl::Initialize::SIGNATURE_HASH,
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
            // Algebra (THENA Fusion, решения 80, 81): фабрика эмитит Pool(token0, token1, pool) при создании пула.
            Proto::Algebra | Proto::AlgebraIntegral | Proto::AlgebraDirFee => {
                ("dex.factory", IAlgebraFactory::Pool::SIGNATURE_HASH, false)
            }
            // Решение 470: Aero Basic — PoolCreated(token0 indexed, token1 indexed, pool, n) без stable (metadex-public 1.1.0
            // IPoolFactory.sol L53, эмит PoolFactory.sol L343).
            Proto::AeroBasic => (
                "dex.factory",
                IAeroBasicFactory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            // Solidly (Topaz v2, THENA V1; решения 80, 85): фабрика эмитит PoolCreated(token0, token1, stable, pool, n)
            // при создании пула (velodrome-finance PoolFactory.sol).
            Proto::Solidly => (
                "dex.factory",
                ISolidlyFactory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            // Solidly V1 — THENA V1 (решение 122 F): PairCreated(token0, token1, stable, pair, n), stable не индексирован
            // (PairFactoryUpgradeable.sol L32, THENA-Contracts f5bc742).
            Proto::SolidlyV1 => (
                "dex.factory",
                IThenaV1Factory::PairCreated::SIGNATURE_HASH,
                false,
            ),
            // Slipstream (Topaz v3; решения 80, 85): CLFactory эмитит PoolCreated(token0, token1, tickSpacing, pool)
            // при создании пула (velodrome-finance/slipstream). Пул — отдельный контракт (как V3).
            Proto::Slipstream => (
                "dex.factory",
                ICLFactory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            // StableSwap (Ellipsis; решение 122 F): фабрики с событием создания нет — новые пулы приходят только из
            // справочника и списка dex.stable_pools.
            Proto::CurveStable => continue,
            // Curve StableSwap-NG (решение 275): событие фабрики PlainPoolDeployed адреса пула не содержит
            // (CurveStableSwapFactoryNG.vy L52-L56) — новые пулы приходят из справочника индексатора.
            Proto::CurveNg => continue,
            // Crypto-пулы Curve (решение 453): события фабрик TwocryptoPoolDeployed / TricryptoPoolDeployed (адрес пула —
            // неиндексированное поле) и CryptoPoolDeployed фабрики v1 (адреса нет — поиск по фабрике на блоке события),
            // created_crypto.rs. У tricrypto 2021 фабрики нет — пул из dex.stable_pools.
            p @ (Proto::CurveTwocrypto
            | Proto::CurveTricrypto
            | Proto::CurveCryptoV1
            | Proto::CurveTricryptoV1) => match created_crypto::topic(p) {
                Some(t) => ("dex.factory", t, false),
                None => continue,
            },
            // Metric OMM (решение 285): событие создания пула фабрики BSC — не установлено (createPool BSC ≠ SDK, исходника
            // нет) — на BSC подписки нет, как раньше. Решение 482 (ревью t251): подпись PoolCreated подтверждена только
            // исходником фабрики сети Arbitrum One (Sourcify, chainId 42161) — подписка только в этой сети
            // (dex.evm_chain_id площадки) и только у DEX без справочника индексатора (indexer_gap.rs).
            Proto::MetricOmm if metric_created(ctx, f.id) => (
                "dex.factory",
                IMetricOmmFactory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            Proto::MetricOmm => continue,
            // iZiSwap / Biswap V3 (решение 286): подписка на NewPool фабрики не подключена (техдолг, docs/ROADMAP.md) —
            // новые пулы приходят из справочника индексатора.
            Proto::IziSwap | Proto::BiswapV3 => continue,
            // Balancer V2 (решение 289): пул регистрирует хранилище (PoolRegistered — от адреса Vault, токены — отдельным
            // TokensRegistered, IVault.sol L235, L273); подписка на них не подключена (техдолг, docs/ROADMAP.md) — новые
            // пулы приходят из справочника индексатора.
            Proto::BalancerV2 => continue,
            // Balancer V3 (решение 452): пул регистрирует хранилище — PoolRegistered от адреса Vault (IVaultEvents.sol
            // L26-L35); кандидат — events_balancer_v3::registered.
            Proto::BalancerV3 => ("dex.vault", events_balancer_v3::registered_topic(), false),
            // Liquidity Book V2.1 (решение 290): событие фабрики LBPairCreated(tokenX, tokenY, binStep — все indexed,
            // LBPair, pid) (ILBFactory.sol L49-L51). Решение 482: подписка у lb_v21 (Swapline V1 на Arbitrum нет у
            // индексатора); у lb_v22 — не подключена (фабрика V2.2 в этом решении не сверялась).
            // Ревью t251: только у DEX без справочника индексатора (LFJ BSC и др. — как раньше, без подписки).
            Proto::LbV21 if indexer_gap::outside(ctx, f.id) == Some(true) => (
                "dex.factory",
                ILBFactory::LBPairCreated::SIGNATURE_HASH,
                false,
            ),
            Proto::LbV21 | Proto::LbV22 => continue,
            // Fluid DEX T1 (решение 440): событие фабрики LogDexDeployed(dex, dexId) токенов не несёт
            // (factory/main.sol L62) — подписка не подключена (техдолг, docs/ROADMAP.md): новые пулы приходят из
            // справочника индексатора.
            Proto::FluidT1 => continue,
            // Скважина Basin (решение 485): BoreWell(well, implementation, tokens, wellFunction, pumps, wellData) фабрики
            // Aquifer — поля не индексированы (IAquifer.sol L42-L44, Aquifer.sol L81-L88). Прокси скважин Beanstalk
            // этим событием не приходят (событие несёт клон за прокси) — они из справочника и dex.stable_pools.
            Proto::BasinWell => (
                "dex.factory",
                IBasinAquifer::BoreWell::SIGNATURE_HASH,
                false,
            ),
            // Shell Protocol Proteus (решение 486): фабрики с событием нет — пулы из списка dex.stable_pools (stable_list.rs).
            Proto::ShellProteus => continue,
            // Liquidity Book v2.0 (решение 445): LBPairCreated фабрики v2.0 (ILBFactory.sol v2.0.0 L26-L32) — подписка не
            // подключена (как у v2.1, техдолг docs/ROADMAP.md): новые пулы — из справочника индексатора.
            Proto::LbV20 => continue,
            // Ekubo (решение 454; раньше — решение 447: только справочник): PoolInitialized ядра (ICore.sol L111) — ключ
            // пула из события, PoolId сверяется (history_ekubo.rs), дальше та же проверка (verify_ekubo.rs, ключ — из
            // памяти, без PoolKeyIndex).
            Proto::Ekubo => (
                "dex.ekubo_core",
                IEkuboCore::PoolInitialized::SIGNATURE_HASH,
                true,
            ),
            // KyberSwap Classic DMM (решение 287): PoolCreated(token0 indexed, token1 indexed, pool, ampBps, totalPool) фабрики
            // (DMMFactory.sol L22-L28, L60).
            Proto::KyberDmm => (
                "dex.factory",
                IDmmFactory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
            // Решение 122 F, 3: PancakeSwap / Lista StableSwap — новые пулы из справочника и поиска по реестру фабрики
            // (getPairInfo / getPairInfos, factory_ask.rs); подписка на NewStableSwapPair — техдолг (docs/ROADMAP.md).
            // Saddle (Nerve) — фабрики нет; WooFi — общий контракт, новые базовые монеты — через поиск по оракулу.
            Proto::PancakeStable
            | Proto::ListaStable
            | Proto::SaddleStable
            | Proto::WooPP
            | Proto::Elfomo
            | Proto::BaiBai
            | Proto::Lunar => {
                continue;
            }
            // Решение 484: Wombat — фабрики нет; пулы — справочник и список dex.stable_pools.
            Proto::Wombat => continue,
            // Maverick V2 (решение 122 F): PoolCreated(poolAddress, …, tokenA, tokenB, kinds, accessor) фабрики, все поля
            // не индексированы (IMaverickV2Factory.sol L35-L47).
            Proto::Maverick => (
                "dex.factory",
                IMaverickV2Factory::PoolCreated::SIGNATURE_HASH,
                false,
            ),
        };
        // Решение 483: варианты Ramses со своей подписью события создания (ramses.rs created_topic).
        let t0 = super::ramses::created_topic(f).unwrap_or(t0);
        if let Ok(addr) = ctx.dex_addr(setting, f.id) {
            out.push((addr, t0, v4, f));
            // Решение 450: у фабрик KyberSwap Classic Static Fee своё событие PoolCreated (abi_dmm.rs created_topics) —
            // подписка на все три подписи; фабрика ключа шлёт одно из них.
            if f.proto == Proto::KyberDmm {
                let extra = abi_dmm::created_topics().into_iter().filter(|t| *t != t0);
                out.extend(extra.map(|t| (addr, t, v4, f)));
            }
        }
    }
    out
}

/// Сеть, в которой подпись PoolCreated фабрики Metric подтверждена исходником: Arbitrum One — Sourcify chainId 42161
/// (MetricOmmPoolFactory 0x2a53…8f32, IMetricOmmPoolFactory.sol; решение 482). Это факт источника, не параметр.
const METRIC_CREATED_CHAIN: u64 = 42161;

/// Подписка на PoolCreated фабрики Metric у DEX: сеть площадки — METRIC_CREATED_CHAIN и DEX без справочника
/// индексатора. Ошибок нет (сеть не задана — нет).
fn metric_created(ctx: &Ctx, id: &str) -> bool {
    ctx.chain_id() == Some(METRIC_CREATED_CHAIN) && indexer_gap::outside(ctx, id) == Some(true)
}

/// Событие → проверенный пул → подписка. Принимает: окружение, наблюдателя, сырой лог.
/// Ошибка — текст (лог не разобран, источник не подключён, проверка не прошла, узел).
pub async fn on_log(p: &Pools, w: &Watch, raw: &str) -> Result<(), String> {
    let log: Log = serde_json::from_str(raw).map_err(|e| format!("лог не разобран: {e}"))?;
    let t0 = log.topics().first().copied().unwrap_or_default();
    // У одного менеджера CL может быть два вида (Infinity CL и Infinity Stable — решение «122 (Infinity Stable)»):
    // событие разбирается каждым подключённым видом этого источника. Нет ни одного — DEX выключили на лету.
    let all: Vec<_> = sources(&p.ctx)
        .into_iter()
        .filter(|(a, t, _, _)| *a == log.inner.address && *t == t0)
        .collect();
    for (_, _, v4, f) in all {
        one(p, w, &log, v4, f).await?;
    }
    Ok(())
}

/// Событие одного вида DEX → проверка → подписка. Ошибка — текст (как у on_log).
async fn one(p: &Pools, w: &Watch, log: &Log, v4: bool, f: &'static Family) -> Result<(), String> {
    // Решение 453: событие фабрики crypto-пулов Curve — до двух пар пула (Tricrypto) или поиск по фабрике (v1).
    if f.proto.is_curve_crypto() {
        let mut errs = Vec::new();
        for c in created_crypto::candidates(&p.ctx, f, log, w.usdt).await? {
            if let Err(why) = take(p, w, f, c).await {
                errs.push(why);
            }
        }
        if errs.is_empty() {
            return Ok(());
        }
        return Err(errs.join("; "));
    }
    let c = if v4 {
        // Infinity Stable: событие пула не с крючком из dex.stable_hooks — не его пул (None).
        let Some(all) = history_ekubo::parse_opt(&p.ctx, f, log).map_err(|e| e.message)? else {
            return Ok(());
        };
        let Some(x) = against(&all, w.usdt) else {
            return Ok(());
        };
        // Проверка V4 идёт по PoolId; у Infinity Stable ссылка — адрес крючка, PoolId — из ключа.
        let pool = match (&x.key, f.proto) {
            (Some(k), Proto::InfinityStable) => format!("{}", k.id()),
            _ => pool_key(&x.reference),
        };
        Candidate {
            family: f,
            pool,
            coin: x.coin,
            quote: w.usdt,
        }
    } else if f.variant==super::family_variant::Variant::Carbon {
        let Some(candidate)=super::carbon_directory::created(&p.ctx,f,log,w.usdt)? else{return Ok(())};candidate
    } else if f.variant == super::family_variant::Variant::RingFew {
        let event =
            IV2Factory::PairCreated::decode_log_data(&log.inner.data).map_err(|e| e.to_string())?;
        let layout = super::ring::active(&p.ctx, f.id).map_err(|e| e.message)?;
        let wq = layout.wrapped(w.usdt);
        let other = if event.token0 == wq {
            event.token1
        } else if event.token1 == wq {
            event.token0
        } else {
            return Ok(());
        };
        let Some(&coin) = w.coins.keys().find(|a| layout.wrapped(**a) == other) else {
            return Ok(());
        };
        Candidate {
            family: f,
            pool: format!("{:#x}", event.pair),
            coin,
            quote: w.usdt,
        }
    } else if f.proto == Proto::BalancerV3 {
        // Решение 452: токены пула Balancer V3 — в tokenConfig события (пул может быть многотокенным).
        match events_balancer_v3::registered(f, log, w.usdt, &w.coins)? {
            Some(c) => c,
            None => return Ok(()),
        }
    } else {
        candidate(f, log, w.usdt)?
    };
    take(p, w, f, c).await
}

/// Кандидат из события → проверка → подписка (монета не из подписки — пропуск). Ошибка — текст (как у on_log).
async fn take(p: &Pools, w: &Watch, f: &'static Family, c: Candidate) -> Result<(), String> {
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
        // Решение 295: у DEX в режиме dex.multi_pair строки одного пула с разными парами — разные (dir_key); у прочих
        // ключ — строка пула, как до решения.
        let key = |x: &IndexedPool| {
            super::multi_pair::dir_key(&s.multi_dex, &x.dex_id, &x.pool, x.base, x.quote)
        };
        let want = key(&item);
        let list = s.events.entry(c.coin).or_default();
        if !list.iter().any(|x| key(x) == want) {
            list.push(item);
        }
    });
}

/// Кандидат из события V2/V3: пул, монета (не встречный токен), встречный токен. Ошибка — лог не по ABI или во
/// встречном поле не тот токен.
pub fn candidate(f: &'static Family, log: &Log, quote: Address) -> Result<Candidate, String> {
    let (t0, t1, pool) = match f.proto {
        _ if f.variant == super::family_variant::Variant::MintClub => {
            let e=super::mint_club::IBond::TokenCreated::decode_log_data(&log.inner.data).map_err(|e|e.to_string())?;
            (e.token,e.reserveToken,e.token)
        }
        _ if f.variant == super::family_variant::Variant::SolidlyCl => {
            let event =
                super::solidly_v3::ISolidlyV3::PoolCreated::decode_log_data(&log.inner.data)
                    .map_err(|err| format!("Solidly V3 PoolCreated не по ABI: {err}"))?;
            (event.token0, event.token1, event.pool)
        }
        // Решение 483: Ramses CL V2 (PoolCreated Uniswap V3) и Ramses Legacy (PairCreated без stable) — ramses.rs.
        _ if super::ramses::created_topic(f).is_some() => super::ramses::created(f, log)?,
        Proto::V2 | Proto::NomiStable | Proto::Smardex | Proto::CamelotV2 => {
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
        // Algebra (THENA Fusion, решения 80, 81): событие Pool(token0, token1, pool) — IAlgebraFactory.sol.
        Proto::Algebra | Proto::AlgebraIntegral | Proto::AlgebraDirFee => {
            let e = IAlgebraFactory::Pool::decode_log_data(&log.inner.data)
                .map_err(|e| format!("Pool (Algebra) не по ABI: {e}"))?;
            (e.token0, e.token1, e.pool)
        }
        // Solidly (Topaz v2, THENA V1; решения 80, 85): PoolCreated(token0, token1, stable, pool, n) — реальная пара
        // и адрес пула из события (velodrome-finance IPoolFactory.sol). Флаг stable читает позже load_solidly.rs.
        Proto::Solidly => {
            let e = ISolidlyFactory::PoolCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PoolCreated (Solidly) не по ABI: {e}"))?;
            (e.token0, e.token1, e.pool)
        }
        // Решение 470: Aero Basic — PoolCreated(token0, token1, pool, n); кривую читает load_aero_basic.rs (POOL_TYPE).
        Proto::AeroBasic => {
            let e = IAeroBasicFactory::PoolCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PoolCreated (Aero Basic) не по ABI: {e}"))?;
            (e.token0, e.token1, e.pool)
        }
        Proto::SolidlyV1 => {
            let e = IThenaV1Factory::PairCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PairCreated (Solidly V1) не по ABI: {e}"))?;
            (e.token0, e.token1, e.pair)
        }
        // Slipstream (Topaz v3; решения 80, 85): PoolCreated(token0, token1, int24 tickSpacing, pool) — третий
        // индексируемый параметр tickSpacing (а не fee как у Uni V3), адрес пула и реальная пара из события.
        Proto::Slipstream => {
            let e = ICLFactory::PoolCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PoolCreated (Slipstream) не по ABI: {e}"))?;
            (e.token0, e.token1, e.pool)
        }
        // Решение 450: событие DMMFactory, KSFactory или KSFactory Legacy — по topic0 (abi_dmm.rs created).
        Proto::KyberDmm => {
            let topic = log.topics().first().copied().unwrap_or_default();
            abi_dmm::created(topic, &log.inner.data)?
        }
        // Решение 485: BoreWell Aquifer — скважина и её токены (функции ConstantProduct2 / Stable2 — только два токена).
        Proto::BasinWell => {
            let e = IBasinAquifer::BoreWell::decode_log_data(&log.inner.data)
                .map_err(|e| format!("BoreWell (Basin) не по ABI: {e}"))?;
            if e.tokens.len() != 2 {
                return Err(format!("BoreWell: скважина из {} токенов", e.tokens.len()));
            }
            (e.tokens[0], e.tokens[1], e.well)
        }
        Proto::Maverick => {
            let e = IMaverickV2Factory::PoolCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PoolCreated (Maverick V2) не по ABI: {e}"))?;
            (e.tokenA, e.tokenB, e.poolAddress)
        }
        // Решение 482: Liquidity Book V2.1 — пул и токены из LBPairCreated (tokenX, tokenY — indexed).
        Proto::LbV21 => {
            let e = ILBFactory::LBPairCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("LBPairCreated не по ABI: {e}"))?;
            (e.tokenX, e.tokenY, e.LBPair)
        }
        // Решение 482: Metric OMM — пул и токены из PoolCreated (все три — indexed); порядок токенов пула не по адресу —
        // монету и встречный разбирает код ниже, проверка — verify_metric.rs.
        Proto::MetricOmm => {
            let e = IMetricOmmFactory::PoolCreated::decode_log_data(&log.inner.data)
                .map_err(|e| format!("PoolCreated (Metric) не по ABI: {e}"))?;
            (e.token0, e.token1, e.poolAddress)
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
