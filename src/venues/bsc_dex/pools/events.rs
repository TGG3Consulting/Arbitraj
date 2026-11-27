//! Файл: venues/bsc_dex/pools/events.rs
//! Что делает: общий итог применения события пула и выбор разбора по виду пула; topic0 событий для подписки
//! (все события — глубоким пулам, только добавление ликвидности — мелким).
//! Кто вызывает: pools/listen_log.rs.
//! От чего зависит: ramses.rs (решение 483: варианты Ramses), events_shell.rs (решение 486), events_v2.rs, events_v3.rs, events_cl.rs, events_bin.rs, events_ekubo.rs (решение 447), events_wombat.rs (решение 484), families.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy_primitives::B256;

use super::events_algebra_dir;
use super::families::{Family, Proto};
use super::registry::Entry;
use super::{
    events_algebra, events_algebra_integral, events_balancer, events_balancer_v3, events_basin,
    events_bin, events_camelot, events_cl, events_crypto, events_curve, events_curve_ng,
    events_dmm, events_dodo, events_ekubo, events_elastic, events_fluid, events_izi, events_lb,
    events_lb20, events_lb22, events_maverick, events_metric, events_nomi, events_shell,
    events_smardex, events_solidly, events_stable_u, events_v2, events_v3, events_wombat,
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
    /// Решение 325: событие меняет только комиссию пула (Algebra Integral: Fee, SwapFee / Swap 1.2.x с добавкой
    /// плагина к lastFee) — перечитать только её (integral_fee.rs) при dex.event_refresh_fee_only; иначе — как Refresh
    /// с этим же текстом (listen_log.rs).
    Fee(String),
}

/// topic0 событий, которые слушаются у пулов этого вида. Ошибок нет.
pub fn topics(f: &Family) -> Vec<B256> {
    use alloy::sol_types::SolEvent;
    match f.proto {
        _ if f.variant.umbrae()=>super::umbrae::topics(f.variant),
        _ if f.variant==super::family_variant::Variant::UniswapV1=>super::uniswap_v1::topics(),
        _ if f.variant==super::family_variant::Variant::RubiconClassic=>super::rubicon::topics(),
        _ if f.variant==super::family_variant::Variant::Carbon=>super::carbon_events::topics(),
        _ if f.variant==super::family_variant::Variant::MintClub => vec![super::mint_club::IBond::Transfer::SIGNATURE_HASH],
        _ if f.variant == super::family_variant::Variant::SolidlyCl => {
            let mut events = events_v3::topics(f.proto);
            events.push(super::solidly_v3::ISolidlyV3::SetFee::SIGNATURE_HASH);
            events
        }
        // Решение 483: варианты Ramses — свои события (FeeAdjustment, Mint с index, Sync uint112; ramses.rs).
        _ if super::ramses::handles(f) => super::ramses::topics(f),
        Proto::V2 => events_v2::topics(),
        // Slipstream (Topaz v3) — форк Uni V3, события пула идентичны (events_v3; topics(Slipstream) даёт Swap Uni V3).
        Proto::PancakeV3 | Proto::UniswapV3 | Proto::Slipstream => events_v3::topics(f.proto),
        Proto::InfinityCl | Proto::UniswapV4 => events_cl::topics(f.proto),
        Proto::InfinityBin => events_bin::topics(),
        Proto::KyberElastic => events_elastic::topics(),
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => events_dodo::topics(),
        Proto::Algebra => events_algebra::topics(),
        Proto::AlgebraIntegral => events_algebra_integral::topics(),
        Proto::AlgebraDirFee => events_algebra_dir::topics(),
        Proto::Solidly | Proto::SolidlyV1 | Proto::AeroBasic => events_solidly::topics(),
        Proto::CurveStable => events_curve::topics_of(f.variant),
        Proto::CurveNg => events_curve_ng::topics(),
        // Crypto-пулы Curve (решения 442, 453: NG и v1): любое событие пула — перечитать (events_crypto.rs).
        Proto::CurveTwocrypto
        | Proto::CurveTricrypto
        | Proto::CurveCryptoV1
        | Proto::CurveTricryptoV1 => events_crypto::topics(),
        Proto::NomiStable => events_nomi::topics(),
        Proto::MetricOmm => events_metric::topics(),
        Proto::IziSwap | Proto::BiswapV3 => events_izi::topics(f.proto),
        Proto::KyberDmm => events_dmm::topics(),
        Proto::Smardex => events_smardex::topics(),
        Proto::BalancerV2 => events_balancer::topics(f),
        Proto::BalancerV3 => events_balancer_v3::topics(),
        Proto::LbV21 => events_lb::topics(),
        Proto::FluidT1 => events_fluid::topics(),
        // Решение 485: скважина Basin (events_basin.rs).
        Proto::BasinWell => events_basin::topics(),
        // Shell Protocol Proteus (решение 486): любое событие пула — перечитать (events_shell.rs).
        Proto::ShellProteus => events_shell::topics(),
        Proto::CamelotV2 => events_camelot::topics(),
        Proto::LbV20 => events_lb20::topics(),
        // Решение 446: LB V2.2 — те же события и HooksParametersSet (events_lb22.rs).
        Proto::LbV22 => events_lb22::topics(),
        Proto::Ekubo if f.variant==super::family_variant::Variant::EkuboV2 => vec![super::ekubo_v2::position_topic(), events_ekubo::SWAP_MARK],
        Proto::Ekubo => events_ekubo::topics(),
        // PancakeSwap / Lista StableSwap, Saddle, WooFi — решение 122 F, 3; Infinity Stable (события крючка) —
        // решение «122 (Infinity Stable)» (events_stable_u.rs).
        Proto::PancakeStable
        | Proto::ListaStable
        | Proto::SaddleStable
        | Proto::WooPP
        | Proto::Elfomo
        | Proto::BaiBai
        | Proto::Lunar
        | Proto::InfinityStable => events_stable_u::topics(f.proto),
        Proto::Maverick => events_maverick::topics(),
        // Решение 484: Wombat — любое событие пула → перечитать (events_wombat.rs).
        Proto::Wombat => events_wombat::topics(),
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
        _ if f.variant.umbrae()=>super::umbrae::topics(f.variant),
        _ if f.variant==super::family_variant::Variant::UniswapV1=>super::uniswap_v1::topics(),
        _ if f.variant==super::family_variant::Variant::RubiconClassic=>super::rubicon::topics(),
        _ if f.variant==super::family_variant::Variant::Carbon => super::carbon_events::topics(),
        _ if f.variant==super::family_variant::Variant::MintClub => vec![super::mint_club::IBond::Transfer::SIGNATURE_HASH],
        _ if super::ramses::handles(f) => super::ramses::liquidity_topics(f),
        Proto::V2 => vec![IV2Pair::Mint::SIGNATURE_HASH],
        Proto::PancakeV3 | Proto::UniswapV3 | Proto::Slipstream => {
            vec![IV3Pool::Mint::SIGNATURE_HASH]
        }
        Proto::InfinityCl => vec![IInfinityCl::ModifyLiquidity::SIGNATURE_HASH],
        Proto::UniswapV4 => vec![IUniswapV4::ModifyLiquidity::SIGNATURE_HASH],
        Proto::InfinityBin => vec![IInfinityBin::Mint::SIGNATURE_HASH],
        Proto::KyberElastic => events_elastic::liquidity_topics(),
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => events_dodo::liquidity_topics(),
        Proto::Algebra => events_algebra::liquidity_topics(),
        Proto::AlgebraIntegral => events_algebra_integral::liquidity_topics(),
        Proto::AlgebraDirFee => events_algebra_dir::liquidity_topics(),
        Proto::Solidly | Proto::SolidlyV1 | Proto::AeroBasic => events_solidly::liquidity_topics(),
        Proto::CurveStable => events_curve::liquidity_topics(),
        Proto::CurveNg => events_curve_ng::liquidity_topics(),
        Proto::CurveTwocrypto
        | Proto::CurveTricrypto
        | Proto::CurveCryptoV1
        | Proto::CurveTricryptoV1 => events_crypto::liquidity_topics(),
        Proto::NomiStable => events_nomi::liquidity_topics(),
        Proto::MetricOmm => events_metric::liquidity_topics(),
        Proto::IziSwap | Proto::BiswapV3 => events_izi::liquidity_topics(f.proto),
        Proto::KyberDmm => events_dmm::liquidity_topics(),
        Proto::Smardex => events_smardex::liquidity_topics(),
        Proto::BalancerV2 => events_balancer::liquidity_topics(),
        Proto::BalancerV3 => events_balancer_v3::liquidity_topics(),
        Proto::LbV21 | Proto::LbV22 => events_lb::liquidity_topics(),
        Proto::FluidT1 => events_fluid::liquidity_topics(),
        Proto::BasinWell => events_basin::liquidity_topics(),
        Proto::ShellProteus => events_shell::liquidity_topics(),
        Proto::CamelotV2 => events_camelot::liquidity_topics(),
        Proto::LbV20 => events_lb20::liquidity_topics(),
        Proto::Ekubo if f.variant==super::family_variant::Variant::EkuboV2 => vec![super::ekubo_v2::position_topic()],
        Proto::Ekubo => events_ekubo::liquidity_topics(),
        Proto::PancakeStable
        | Proto::ListaStable
        | Proto::SaddleStable
        | Proto::WooPP
        | Proto::Elfomo
        | Proto::BaiBai
        | Proto::Lunar
        | Proto::InfinityStable => events_stable_u::liquidity_topics(f.proto),
        Proto::Maverick => events_maverick::liquidity_topics(),
        Proto::Wombat => events_wombat::liquidity_topics(),
    }
}

/// Применяет событие к записи. Принимает: запись, лог. Возвращает: Applied. Ошибок нет (разбор не удался → Refresh).
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    use super::solidly_v3::ISolidlyV3::SetFee;
    use alloy::sol_types::SolEvent;
    let r = match e.family.proto {
        _ if e.family.variant.umbrae() => {e.valid=false;Applied::Refresh("Обновлено состояние рынка Umbrae".into())}
        _ if e.family.variant.launch() => {e.valid=false;Applied::Refresh("Изменились условия обмена launchpad или состояние внешних контрактов".into())}
        _ if e.family.variant.deli() => {e.valid=false;Applied::Refresh("Изменили состояние Deli и внешних контрактов".into())}
        _ if e.family.variant==super::family_variant::Variant::Bundles => {e.valid=false;Applied::Refresh("Изменились резервы Bundles Swap".into())}
        _ if e.family.variant==super::family_variant::Variant::Antfarm => {e.valid=false;Applied::Refresh("Изменилось состояние пары Antfarm".into())}
        _ if e.family.variant==super::family_variant::Variant::MaverickOne=>{e.valid=false;Applied::Refresh("Изменилось состояние Maverick V1".into())}
        _ if e.family.variant==super::family_variant::Variant::Surge=>{e.valid=false;Applied::Refresh("Обновилось состояние Surge".into())}
        _ if e.family.variant==super::family_variant::Variant::CypherIntegral=>{e.valid=false;Applied::Refresh("Изменились пул или плагин Cypher".into())}
        _ if e.family.variant.protected_router_wallet()=>{e.valid=false;Applied::Refresh("Изменились условия защищённого обмена".into())}
        _ if e.family.variant==super::family_variant::Variant::IntegralSize=>{e.valid=false;Applied::Refresh("Изменилось состояние пары Integral SIZE".into())}
        _ if e.family.variant==super::family_variant::Variant::Pendle=>{e.valid=false;Applied::Refresh("Изменилось состояние рынка Pendle".into())}
        _ if e.family.variant==super::family_variant::Variant::Dagrow=>{ e.valid=false;Applied::Refresh("Обновилось состояние DAGrow SSP".into()) }
        _ if e.family.variant==super::family_variant::Variant::Temple => {e.valid=false;Applied::Refresh("Изменились резервы TempleDAO".into())}
        _ if e.family.variant==super::family_variant::Variant::UniswapV1=>{
            e.valid=false;Applied::Refresh("Uniswap V1: изменились физические резервы биржи".into())
        }
        _ if e.family.variant==super::family_variant::Variant::RubiconClassic => {
            let pair=log.topics().get(2);
            let a=e.pool.token0;let b=e.pool.token1;
            if [alloy_primitives::keccak256([a.as_slice(),b.as_slice()].concat()),alloy_primitives::keccak256([b.as_slice(),a.as_slice()].concat())].iter().any(|key|Some(key)==pair){
                e.valid=false;Applied::Refresh("Изменились заявки Rubicon Classic".into())
            } else {Applied::Ignored}
        }
        _ if e.family.variant==super::family_variant::Variant::Carbon => {e.valid=false;Applied::Refresh("Заявки или плата Carbon изменились".into())}

        _ if e.family.variant==super::family_variant::Variant::MintClub => {
            match super::mint_club::IBond::Transfer::decode_log_data(&log.inner.data) {
                Ok(t) if !t.from.is_zero() && !t.to.is_zero() => Applied::Ignored,
                _ => {e.valid=false;Applied::Refresh("Выпуск или погашение Mint Club требует актуальной кривой и обеспечения".into())}
            }
        }
        // Sync содержит только резервы. Setter платы может пройти в том же блоке без события.
        Proto::V2 if e.family.variant.v2_block_state() => {
            e.valid = false;
            Applied::Refresh("Резервы и изменяемая комиссия V2 требуют одного блока".into())
        }
        _ if e.family.variant == super::family_variant::Variant::SolidlyCl
            && log.topics().first() == Some(&SetFee::SIGNATURE_HASH) =>
        {
            match SetFee::decode_log_data(&log.inner.data) {
                Ok(change)
                    if change.feeNew.to::<u32>() <= 100_000
                        && e.pool.fee_pips == Some(change.feeOld.to::<u32>()) =>
                {
                    e.pool.fee_pips = Some(change.feeNew.to());
                    Applied::Updated
                }
                _ => {
                    e.valid = false;
                    Applied::Refresh("Solidly V3 SetFee расходится с состоянием".into())
                }
            }
        }
        _ if super::ramses::handles(e.family) => super::ramses::apply(e, log),
        Proto::V2 => events_v2::apply(e, log),
        Proto::PancakeV3 | Proto::UniswapV3 | Proto::Slipstream => events_v3::apply(e, log),
        Proto::InfinityCl | Proto::UniswapV4 => events_cl::apply(e, log),
        Proto::InfinityBin => events_bin::apply(e, log),
        Proto::KyberElastic => events_elastic::apply(e, log),
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => events_dodo::apply(e, log),
        Proto::Algebra => events_algebra::apply(e, log),
        Proto::AlgebraIntegral => events_algebra_integral::apply(e, log),
        Proto::AlgebraDirFee => events_algebra_dir::apply(e, log),
        Proto::Solidly | Proto::SolidlyV1 | Proto::AeroBasic => events_solidly::apply(e, log),
        Proto::CurveStable => events_curve::apply(e, log),
        Proto::CurveNg => events_curve_ng::apply(e, log),
        Proto::CurveTwocrypto
        | Proto::CurveTricrypto
        | Proto::CurveCryptoV1
        | Proto::CurveTricryptoV1 => events_crypto::apply(e, log),
        Proto::NomiStable => events_nomi::apply(e, log),
        Proto::MetricOmm => events_metric::apply(e, log),
        Proto::IziSwap | Proto::BiswapV3 => events_izi::apply(e, log),
        Proto::KyberDmm => events_dmm::apply(e, log),
        Proto::Smardex => events_smardex::apply(e, log),
        Proto::BalancerV2 => events_balancer::apply(e, log),
        Proto::BalancerV3 => events_balancer_v3::apply(e, log),
        Proto::LbV21 => events_lb::apply(e, log),
        Proto::FluidT1 => events_fluid::apply(e, log),
        Proto::BasinWell => events_basin::apply(e, log),
        Proto::ShellProteus => events_shell::apply(e, log),
        Proto::CamelotV2 => events_camelot::apply(e, log),
        Proto::LbV20 => events_lb20::apply(e, log),
        Proto::LbV22 => events_lb22::apply(e, log),
        Proto::Ekubo if e.family.variant==super::family_variant::Variant::EkuboV2 => {
            if super::ekubo_route::singleton(log)==Some(e.pool.reference.clone()) &&
                (log.topics().is_empty() || log.topic0()==Some(&super::ekubo_v2::position_topic())) {
                Applied::Refresh("Ekubo V2: изменились цена или позиция; перечитать каноническое состояние".into())
            } else { Applied::Ignored }
        }
        Proto::Ekubo => events_ekubo::apply(e, log),
        Proto::PancakeStable
        | Proto::ListaStable
        | Proto::SaddleStable
        | Proto::WooPP
        | Proto::Elfomo
        | Proto::BaiBai
        | Proto::Lunar
        | Proto::InfinityStable => events_stable_u::apply(e, log),
        Proto::Maverick => events_maverick::apply(e, log),
        Proto::Wombat => events_wombat::apply(e, log),
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
