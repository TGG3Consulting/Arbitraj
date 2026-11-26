//! Файл: venues/bsc_dex/pools/per_block.rs
//! Что делает: перечитывание пулов, чьё состояние меняется БЕЗ событий самого пула (решение 122 F, 3), на каждом новом
//! блоке потока голов (node/heads.rs) — это событие «новый блок», а не опрос по таймеру (CLAUDE.md, ценность 1):
//! • WooFi: цену/спред/коэффициент меняет оракул (postState/postPrice и сжатые данные в fallback оракула — событий у
//!   оракула нет), свежесть цены оракул считает по времени блока;
//! • Lista StableSwap: проверка цены пула после обмена сравнивает с внешним оракулом (resilient oracle), рампа A — по
//!   времени блока;
//! • PancakeSwap Infinity Stable (решение «122 (Infinity Stable)»): курсы монет с оракулом курса или ERC4626 крючок
//!   берёт внешними вызовами на каждом обмене (stored_rates), вид монеты не читается — перечитывается всегда; рампа A —
//!   по времени блока.
//! • Metric OMM (решение 285): цена провайдера меняется каждым блоком (лесенка ответов пула — на каждом блоке).
//! • iZiSwap / Biswap V3 (решение 286): лесенка ответов пула; у Biswap V3 комиссия голосуемая, событие
//!   ChangeLiquidityState не слушается — перечитывание на каждом блоке (и на события пула, events_izi.rs). Ревью t118:
//!   только пулы с USDT в паре (each_block) — у остальных лесенки нет никогда, перечитывать нечего. Решение 306: при
//!   dex.pmm_ladder_counter = да у DEX лесенка есть и у пар не с USDT — они тоже каждым блоком (ladder_counter.rs).
//! • Nomiswap Stable (решение 284): setSwapFee пары события не шлёт, A — рампа по времени блока (getA).
//! • KyberSwap Classic DMM (решение 287): комиссия getTradeInfo считается по номеру блока (EMA объёма) — меняется
//!   каждым блоком без событий пула; котировку получает каждый пул (своя формула), поэтому перечитываются все.
//! • SmarDex (решение 288): средняя цена пары зависит от времени блока, резерв — от баланса пары (перевод в пару события
//!   не шлёт); котировку получает каждая пара — перечитываются все.
//! • Balancer V2 (решение 289): балансы пула меняет хранилище своими событиями (Swap, PoolBalanceChanged,
//!   PoolBalanceManaged — лог от адреса Vault с id пула, Swaps.sol L297, PoolBalances.sol), подписка по адресу пула их
//!   не видит; A стейблсвапа во время рампы меняется по времени блока без событий (StablePool.sol L628-L651) —
//!   перечитываются все пулы — одной пачкой на блок (ревью 289, per_block_balancer.rs).
//! • Fluid DEX T1 (решение 440): центральная цена и диапазон сдвигаются по времени, проценты слоя ликвидности
//!   начисляются по времени, операции других пользователей слоя ликвидности событий пула не шлют — перечитываются все.
//! • Liquidity Book V2.1 (решение 290): изменчивая часть комиссии считается от времени блока обмена
//!   (PairParameterHelper.updateReferences L412-L421) — состояние всех пулов одной пачкой на блок (per_block_lb.rs).
//! • THENA Fusion (решение 463): комиссия следующего блока зависит от времени — на каждой голове пересчитывается
//!   локально по прочитанным данным оракула, без узла (fusion_head.rs); пул каждым блоком не перечитывается; у края
//!   горизонта — чтение оракула, одновременно с чтениями остальных видов (ревью t236).
//! Ещё — события общего контракта WooFi (WooSwap, Upgraded, Paused…): ссылка пар — PoolRef::Member (контракт,
//! монета), лог приходит от адреса контракта — перечитываются все пары этого контракта на блоке события.
//! Решение 295: пары многомонетного пула (PoolRef::Multi) видов «каждым блоком» (Curve NG) — одно чтение на контракт
//! (reload_many::load_group → load_site.rs); Balancer — per_block_balancer.rs (тоже по контракту).
//! Решение 330: при dex.block_read_parallel_max — пулы блока читаются одновременно (per_block_par.rs: свои пулы видов
//! «по одному» и пары многомонетных — одним чтением группы с пределом, Balancer и LB — одновременно с ними), пары
//! общего контракта по логу (on_member_log) — одним чтением группы с тем же пределом. Не задана — как раньше, по очереди.
//! Решение 339 (pool_fail.rs): при dex.pool_fail_pause_ms не прочитавшийся пул отмечается с причиной (экран «Монеты»,
//! журнал) и следующие блоки / логи его не читают, пока не прошла растущая пауза; не задана — как раньше, каждым блоком.
//! Задача t160 (dex.per_block_skip_same, reload.rs): чтение каждым блоком, не изменившее состояние пула, ядру не шлётся
//! (load_one_block, reload_many::load_group_block, put_emit в per_block_balancer.rs / per_block_lb.rs); пары общего
//! контракта по логу (on_member_log) — как раньше. Не задана — как раньше.
//! • Решение 453: crypto-пулы Curve в рампе A/gamma (A и gamma — по времени блока) — пока последнее чтение видит
//!   рампу (CryptoData.moving, ramping); ревью 453 — и tricrypto 2021, пока is_killed (unkill_me без события;
//!   per_block_crypto.rs killed).
//! • Решение 472: Curve 2021 (curve_stableswap_plain — 2pool Arbitrum, Plain*Basic фабрики): рампа A по времени
//!   блока, у 2pool donate_admin_fees без события — перечитываются все.
//! • Решение 455: crypto-пулы Curve с переводом токена на пул без события пула (transfer_in.rs due) — один раз, на
//!   первой голове после перевода, если чтение его ещё не учло.
//! • Решение 484: Wombat — mintFee(token) любым адресом меняет cash / liability актива без события пула,
//!   setCovRatioFeeParam — без события, цены активов динамического пула (getRelativePrice) — внешние: перечитываются все.
//! • Решение 470 (T470-07): пулы Aero Slipstream V3 — ЛЁГКОЕ чтение (light): только комиссия крючка пачкой
//!   (per_block_aero.rs); цена, ликвидность и тики — по событиям пула полным чтением.
//! Кто вызывает: pools/session.rs, session_lanes.rs (ramping), check/memory_proof_rules.rs (unevented).
//! От чего зависит: listen.rs (Pools, End), reload.rs (load_one, load_one_block), reload_many.rs (load_group_block,
//! load_group_with),
//! per_block_par.rs, load_batch_par.rs (limit), pool_fail.rs, discover.rs (Found), families.rs, ladder_counter.rs,
//! per_block_crypto.rs (решение 453), transfer_in.rs (решение 455),
//! refresh_urgency.rs (решение 314: лог общего контракта — класс очереди узла, только при парах контракта).
//! Настройки: dex.pmm_ladder_counter, dex.pmm_ladder_usd [DEX] (ladder_counter.rs each_block); dex.event_refresh_urgency
//! (on_member_log); dex.block_read_parallel_max [bsc_dex] (решение 330); через вызываемые — dex.*.

use alloy::rpc::types::Log;
use alloy_primitives::Address;

use crate::core::types::PoolRef;

use super::discover::Found;
use super::families::{Family, Proto};
use super::family_variant::Variant;
use super::listen::{End, Pools};
use super::{
    ladder_counter, load_batch_par, per_block_aero, per_block_balancer, per_block_lb,
    per_block_par, pool_fail, reload, reload_many,
};

/// Решения 453, 455: crypto-пулы Curve без событий пула (рампа A/gamma, остановленный tricrypto 2021) —
/// per_block_crypto.rs; прежние пути вызова сохранены.
pub use super::per_block_crypto::{any_ramping, killed, moving, ramping, unevented};

/// Настройка предела одновременных чтений пулов блока, пар общего контракта и приёма / повтора (решение 330).
pub const PAR: &str = "dex.block_read_parallel_max";

/// Перечитывается ли вид на каждом блоке целиком. Решение 472: и Curve 2021 (вариант CurvePlain вида
/// curve_stableswap_plain — рампа A по времени блока без событий, у 2pool donate_admin_fees меняет balances без
/// события). Ошибок нет.
pub fn needs(f: &Family) -> bool {
    // Решение 472 (доделка): метапул — курс LP базового пула (get_virtual_price) меняется без событий метапула;
    // Plain*Balances — балансы по balanceOf (переводы на пул без события пула).
    // Решение 481: пул с курсом оракула (ETH/wstETH) — latestAnswer() оракула меняется без событий пула.
    if matches!(
        f.variant,
        super::family_variant::Variant::SlipstreamDynamicFee
            | Variant::Memebox
            | Variant::CandySwap
            | Variant::V2Create2LiveFee
            | Variant::DiamondV2
            | Variant::LeetSwap
            | Variant::SolidlyClassic | Variant::SolidlyGenerator
            | Variant::EtherVista
            | Variant::Oasis
            | Variant::DeltaSwap
            | Variant::FraxTwamm
            | Variant::FraxTwammRouter
            | Variant::BasePluginV1 | Variant::TrebleIntegral
            | Variant::NamiIntegral | Variant::SupernovaIntegral
            | super::family_variant::Variant::QuickswapIntegral
            | super::family_variant::Variant::CurvePlain
            | super::family_variant::Variant::CurveMeta
            | super::family_variant::Variant::CurveOracle
    ) {
        return true;
    }
    matches!(
        f.proto,
        Proto::ListaStable
            | Proto::WooPP
            | Proto::Elfomo
            | Proto::BaiBai
            | Proto::Lunar
            | Proto::InfinityStable
            | Proto::CurveNg
            | Proto::NomiStable
            | Proto::MetricOmm
            | Proto::IziSwap
            | Proto::BiswapV3
            | Proto::KyberDmm
            | Proto::Smardex
            | Proto::BalancerV2
            | Proto::BalancerV3
            | Proto::LbV21
            | Proto::FluidT1
            | Proto::ShellProteus
            | Proto::LbV20
            | Proto::LbV22
            | Proto::Wombat
    )
}

/// Решение 470 (T470-07): лёгкое чтение каждым блоком — только комиссия крючка (Aero Slipstream V3, вариант
/// SlipstreamAero: per_block_aero.rs); состояние пула — по событиям. Ошибок нет.
pub fn light(f: &Family) -> bool {
    matches!(
        f.variant,
        Variant::SlipstreamAero | Variant::SlipstreamAeroMetaquoter
    )
}

/// Чтение каждым блоком — полное (needs) или лёгкое (light). Ошибок нет.
pub fn touched(f: &Family) -> bool {
    needs(f) || light(f)
}

/// Есть ли в пачке такие пулы. Ошибок нет.
pub fn any(pools: &[(Found, String)]) -> bool {
    pools.iter().any(|(f, _)| touched(f.family)) || super::fusion_head::any(pools)
}

/// Перечитывать ли этот пул на каждом блоке. Ревью t118 (решение 286): пул iZiSwap / Biswap V3 без USDT в паре лесенки
/// не получает никогда (load_metric_ladder.rs read_with: котируемая не USDT — пусто) — каждым блоком не перечитывается
/// (состояние без лесенки = «без котировки», устаревать нечему; события пула его по-прежнему перечитывают). USDT не
/// задан — так же (лесенки нет). Прочие виды (Metric, WooFi …) — как раньше, по needs. Принимает: пул, USDT. Ошибок нет.
pub fn each_block(f: &Found, usdt: Option<Address>) -> bool {
    if !needs(f.family) {
        return false;
    }
    if f.family.proto.is_izi() {
        return usdt.is_some_and(|u| f.token0 == u || f.token1 == u);
    }
    true
}

/// Новый блок: перечитать на нём прочитанные пулы этих видов (не прочитанные ведёт reload::retry). Не прочитался —
/// пул недостоверен (valid = false) до следующего удачного чтения. Решение 463 (ревью t236, п. 3): комиссия THENA
/// Fusion (fusion_head.rs: пересчёт без узла, у края горизонта — чтение оракула) — одновременно с чтениями остальных
/// видов, а не перед ними. Ошибки: End — ядро закрыло канал (первая из частей).
pub async fn on_head(p: &Pools, pools: &[(Found, String)], block: u64) -> Result<(), End> {
    let (fusion, rest) = tokio::join!(
        super::fusion_head::on_head(p, pools, block),
        others(p, pools, block)
    );
    fusion?;
    rest
}

/// Виды «каждым блоком» без THENA Fusion (см. on_head). Ошибки: End — ядро закрыло канал.
async fn others(p: &Pools, pools: &[(Found, String)], block: u64) -> Result<(), End> {
    let usdt = p.tokens.usdt(&p.ctx).ok();
    let mut variable_v2 = Vec::new();
    let mut balancer = Vec::new();
    let mut lb = Vec::new();
    // Решение 295: пары многомонетного пула (PoolRef::Multi) прочих видов — одно чтение на контракт (sites).
    let mut sites: Vec<(PoolRef, Vec<(&Found, &str)>)> = Vec::new();
    // Решение 306: при dex.pmm_ladder_counter = да — и пулы iZiSwap / Biswap V3 без USDT в паре (ladder_counter.rs).
    // Решение 339: пул на паузе после неудач чтения (pool_fail.rs) в этот блок не читается.
    let per = |f: &Found| {
        (each_block(f, usdt)
            || ladder_counter::each_block(&p.ctx, f, usdt)
            || ramping(p, f)
            || super::transfer_in::due(p, f))
            && !pool_fail::paused(p, &f.reference)
    };
    // Решение 470 (T470-07): пулы Aero Slipstream V3 — лёгкое чтение комиссии пачкой (per_block_aero.rs).
    let aero: Vec<&(Found, String)> = pools
        .iter()
        .filter(|(f, _)| light(f.family) && !pool_fail::paused(p, &f.reference))
        .collect();
    if !aero.is_empty() {
        per_block_aero::on_head(p, &aero, block).await?;
    }
    // Решение 330: задан предел — свои пулы копятся и читаются одновременно (per_block_par.rs); не задан — по одному.
    let par = load_batch_par::limit(&p.ctx, PAR);
    let mut singles: Vec<(&Found, &str)> = Vec::new();
    for x in pools.iter().filter(|(f, _)| per(f)) {
        let (f, t) = x;
        if p.reg.get(&f.reference).is_none() {
            continue;
        }
        if f.family.variant.live_v2_fee() {
            if !p.holds.pending(&f.reference) {
                variable_v2.push((f, t.as_str()));
            }
            continue;
        }
        if matches!(f.reference, PoolRef::Multi { .. }) && f.family.proto != Proto::BalancerV2 {
            let site = f.reference.site();
            match sites.iter_mut().find(|(s, _)| *s == site) {
                Some((_, list)) => list.push((f, t.as_str())),
                None => sites.push((site, vec![(f, t.as_str())])),
            }
            continue;
        }
        // Ревью 289: пулы Balancer V2 — все вместе одной пачкой после остальных (per_block_balancer.rs).
        if f.family.proto == Proto::BalancerV2 {
            balancer.push(x);
            continue;
        }
        // Решение 290: пулы Liquidity Book V2.1 — все вместе одной пачкой (per_block_lb.rs).
        // Решение 445: и Liquidity Book v2.0 — той же пачкой (свои вызовы на пул — lb_head.rs calls_for); решение 446 —
        // и V2.2.
        if matches!(f.family.proto, Proto::LbV21 | Proto::LbV20 | Proto::LbV22) {
            lb.push(x);
            continue;
        }
        if par.is_some() {
            singles.push((f, t.as_str()));
            continue;
        }
        if let Err(why) = reload::load_one_block(p, f, t, block).await? {
            pool_fail::block_failed(p, f, &why).await;
        }
    }
    if !variable_v2.is_empty() {
        let results = reload_many::load_group_block(p, &variable_v2, block, par).await?;
        for ((found, _), result) in variable_v2.into_iter().zip(results) {
            if let Err(why) = result {
                pool_fail::block_failed(p, found, &why).await;
            }
        }
    }
    if let Some(n) = par {
        let lists = per_block_par::Lists {
            singles,
            sites: sites.into_iter().flat_map(|(_, items)| items).collect(),
            balancer: &balancer,
            lb: &lb,
        };
        return per_block_par::on_head(p, lists, block, n).await;
    }
    for (_, items) in sites {
        for ((f, _), r) in items
            .iter()
            .zip(reload_many::load_group_block(p, &items, block, None).await?)
        {
            if let Err(why) = r {
                pool_fail::block_failed(p, f, &why).await;
            }
        }
    }
    per_block_balancer::on_head(p, &balancer, block).await?;
    per_block_lb::on_head(p, &lb, block).await
}

/// Лог общего контракта (пары PoolRef::Member): перечитать на блоке лога все пары контракта. Возвращает: true — лог
/// был у такого контракта. Ошибки: End — ядро закрыло канал.
pub async fn on_member_log(p: &Pools, pools: &[(Found, String)], log: &Log) -> Result<bool, End> {
    let addr = log.address();
    let members: Vec<&(Found, String)> = pools
        .iter()
        .filter(
            |(f, _)| matches!(f.reference, PoolRef::Member { contract, .. } if contract == addr),
        )
        .collect();
    if members.is_empty() {
        return Ok(false);
    }
    // Решение 339: пары на паузе после неудач чтения (pool_fail.rs) по логу не читаются.
    let members: Vec<&(Found, String)> = members
        .into_iter()
        .filter(|(f, _)| !pool_fail::paused(p, &f.reference))
        .collect();
    let Some(block) = log.block_number else {
        return Ok(true);
    };
    // Решение 314 (ревью): класс dex.event_refresh_urgency — только когда есть пары и блок (есть работа узлу).
    // Решение 330: задан предел — все пары контракта одним чтением группы одновременно (тот же блок лога).
    if let Some(n) = load_batch_par::limit(&p.ctx, PAR) {
        let items: Vec<(&Found, &str)> = members.iter().map(|(f, t)| (f, t.as_str())).collect();
        let work = async {
            let res = reload_many::load_group_with(p, &items, block, Some(n)).await?;
            per_block_par::failed(p, &items, &res).await;
            Ok::<bool, End>(true)
        };
        return super::refresh_urgency::urgent(&p.ctx, work).await;
    }
    let work = async {
        for (f, t) in members {
            if let Err(why) = reload::load_one(p, f, t, block).await? {
                pool_fail::block_failed(p, f, &why).await;
            }
        }
        Ok::<bool, End>(true)
    };
    super::refresh_urgency::urgent(&p.ctx, work).await
}
