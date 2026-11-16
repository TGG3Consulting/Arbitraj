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
//! • Liquidity Book V2.1 (решение 290): изменчивая часть комиссии считается от времени блока обмена
//!   (PairParameterHelper.updateReferences L412-L421) — состояние всех пулов одной пачкой на блок (per_block_lb.rs).
//! Ещё — события общего контракта WooFi (WooSwap, Upgraded, Paused…): ссылка пар — PoolRef::Member (контракт,
//! монета), лог приходит от адреса контракта — перечитываются все пары этого контракта на блоке события.
//! Решение 295: пары многомонетного пула (PoolRef::Multi) видов «каждым блоком» (Curve NG) — одно чтение на контракт
//! (reload_many::load_group → load_site.rs); Balancer — per_block_balancer.rs (тоже по контракту).
//! Кто вызывает: pools/session.rs.
//! От чего зависит: listen.rs (Pools, End), reload.rs (load_one), discover.rs (Found), families.rs, ladder_counter.rs,
//! refresh_urgency.rs (решение 314: лог общего контракта — класс очереди узла, только при парах контракта).
//! Настройки: dex.pmm_ladder_counter, dex.pmm_ladder_usd [DEX] (ladder_counter.rs each_block); dex.event_refresh_urgency
//! (on_member_log); через вызываемые — dex.*.

use alloy::rpc::types::Log;
use alloy_primitives::Address;

use crate::core::types::PoolRef;

use super::discover::Found;
use super::families::{Family, Proto};
use super::listen::{End, Pools};
use super::{ladder_counter, per_block_balancer, per_block_lb, reload, reload_many};

/// Перечитывается ли вид на каждом блоке. Ошибок нет.
pub fn needs(f: &Family) -> bool {
    matches!(
        f.proto,
        Proto::ListaStable
            | Proto::WooPP
            | Proto::InfinityStable
            | Proto::CurveNg
            | Proto::NomiStable
            | Proto::MetricOmm
            | Proto::IziSwap
            | Proto::BiswapV3
            | Proto::KyberDmm
            | Proto::Smardex
            | Proto::BalancerV2
            | Proto::LbV21
    )
}

/// Есть ли в пачке такие пулы. Ошибок нет.
pub fn any(pools: &[(Found, String)]) -> bool {
    pools.iter().any(|(f, _)| needs(f.family))
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
/// пул недостоверен (valid = false) до следующего удачного чтения. Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[(Found, String)], block: u64) -> Result<(), End> {
    let usdt = p.tokens.usdt(&p.ctx).ok();
    let mut balancer = Vec::new();
    let mut lb = Vec::new();
    // Решение 295: пары многомонетного пула (PoolRef::Multi) прочих видов — одно чтение на контракт (sites).
    let mut sites: Vec<(PoolRef, Vec<(&Found, &str)>)> = Vec::new();
    // Решение 306: при dex.pmm_ladder_counter = да — и пулы iZiSwap / Biswap V3 без USDT в паре (ladder_counter.rs).
    let per = |f: &Found| each_block(f, usdt) || ladder_counter::each_block(&p.ctx, f, usdt);
    for x in pools.iter().filter(|(f, _)| per(f)) {
        let (f, t) = x;
        if p.reg.get(&f.reference).is_none() {
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
        if f.family.proto == Proto::LbV21 {
            lb.push(x);
            continue;
        }
        if reload::load_one(p, f, t, block).await?.is_err() {
            p.reg.update(&f.reference, |e| e.valid = false);
        }
    }
    for (_, items) in sites {
        for ((f, _), r) in items.iter().zip(reload_many::load_group(p, &items, block).await?) {
            if r.is_err() {
                p.reg.update(&f.reference, |e| e.valid = false);
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
    let Some(block) = log.block_number else {
        return Ok(true);
    };
    // Решение 314 (ревью): класс dex.event_refresh_urgency — только когда есть пары и блок (есть работа узлу).
    let work = async {
        for (f, t) in members {
            if reload::load_one(p, f, t, block).await?.is_err() {
                p.reg.update(&f.reference, |e| e.valid = false);
            }
        }
        Ok::<bool, End>(true)
    };
    super::refresh_urgency::urgent(&p.ctx, work).await
}
