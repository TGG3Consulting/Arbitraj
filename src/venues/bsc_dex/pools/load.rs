//! Файл: venues/bsc_dex/pools/load.rs
//! Что делает: чтение состояния любого пула на заданном блоке (по его виду) и сборка записи реестра
//! из найденного пула; перечитывание записи (после реорга, обрыва или расхождения карты с событием).
//! Кто вызывает: pools/listen.rs.
//! От чего зависит: load_v2v3.rs, load_cl.rs, load_bin.rs, load_lb.rs, load_lb20.rs, load_fluid.rs, registry.rs, discover.rs,
//! tokens.rs.
//! Решение 281: у DEX с dex.v3_fee_source = pool_fee_each_read fee() пула V3 читается с состоянием (fee_fresh.rs) и
//! заменяет комиссию записи (built, renewed).
//! Настройки: через вызываемые файлы (dex.tick_words, dex.bin_depth_usd, dex.tick_lens, dex.state_view, dex.v3_fee_source).

use alloy_primitives::Address;
use chrono::Utc;

use crate::core::types::{Pool, PoolKind, PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::tokens::Tokens;
use super::bins::BinMap;
use super::discover::Found;
use super::families::Family;
use super::families::Proto;
use super::fee_fresh;
use super::hooks::DYNAMIC_FEE_FLAG;
use super::load_algebra_dir;
use super::registry::{Entry, Registry};
use super::ticks::TickMap;
use super::{
    load_algebra, load_algebra_integral, load_balancer, load_balancer_v3, load_bin, load_camelot, load_cl, load_crypto, load_crypto_v1, load_curve, load_curve_ng, load_dmm, load_dodo,
    load_elastic, load_fluid, load_inf_stable, load_izi, load_lb, load_lb20, load_maverick, load_metric, load_nomi, load_saddle, load_slipstream, load_smardex,
    load_solidly, load_stable_u, load_v2v3, load_woo,
};

/// Прочитанное состояние.
pub struct Loaded {
    pub state: PoolState,
    pub ticks: Option<TickMap>,
    pub bins: Option<BinMap>,
    /// Свежая fee() пула V3 на том же блоке (решение 281, fee_fresh.rs; dex.v3_fee_source = pool_fee_each_read).
    /// None — не читалась: в записи остаётся комиссия из проверки пула.
    pub fee_pips: Option<u32>,
}

/// Чтение состояния пула на блоке.
/// Принимает: вид, ссылку, tickSpacing, binStep, USDT и пару (token0, token1) (кто из них Y у Bin; индексы пары у
/// StableSwap), токены (decimals USDT и монет StableSwap), реестр (решение 306: курс встречного токена к USDT для
/// лесенки цен пула пары не с USDT — pools/ladder_counter.rs; другим видам не нужен), блок.
/// Ошибки: не задана нужная настройка, узел, ответ не по ABI, нет tickSpacing/binStep у пула.
#[allow(clippy::too_many_arguments)]
pub async fn state(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    reference: &PoolRef,
    spacing: Option<i32>,
    step: Option<u32>,
    usdt: Address,
    pair: (Address, Address),
    block: u64,
) -> GatewayResult<Loaded> {
    let token1 = pair.1;
    let tag = block_tag(block);
    let missing = |what: &str| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{}: у пула нет {what}", f.title),
        )
    };
    // DODO PMM (решение 86b): вид ядра Pmm, пул — отдельный контракт, чтение своё (load_dodo.rs): кривая
    // getPMMStateForCall + комиссии getUserFeeRate + ориентация base/quote. Карт тиков/бинов нет.
    if f.proto.is_dodo() {
        if let PoolRef::Contract(a) = reference {
            return Ok(Loaded {
                state: load_dodo::load(ctx, *a, &tag).await?,
                ticks: None,
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул DODO не того вида", f.title),
        ));
    }
    // StableSwap (Ellipsis; решение 122 F): вид ядра StableSwap, пул — отдельный контракт, чтение своё (load_curve.rs):
    // coins/balances/A/fee одним Multicall3 + decimals монет. Карт тиков/бинов нет.
    // Решение 295: и пара многомонетного пула (PoolRef::Multi) — тот же контракт, состояние этой пары.
    if f.proto == Proto::CurveStable {
        if let Some(a) = reference.own_contract() {
            return Ok(Loaded {
                state: load_curve::load(ctx, tokens, a, pair, &tag).await?,
                ticks: None,
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул StableSwap не того вида", f.title),
        ));
    }
    // Curve StableSwap-NG (решение 275): вид ядра StableSwap, пул — отдельный контракт, чтение своё (load_curve_ng.rs).
    if f.proto == Proto::CurveNg {
        if let Some(a) = reference.own_contract() {
            return Ok(Loaded {
                state: load_curve_ng::load(ctx, a, pair, &tag).await?,
                ticks: None,
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Curve NG не того вида", f.title),
        ));
    }
    // Curve Twocrypto-NG / Tricrypto-NG (решение 442): вид ядра Crypto, пул — отдельный контракт; состояние и время
    // блока одной пачкой (load_crypto.rs). Tricrypto — и пара многомонетного пула (PoolRef::Multi).
    if matches!(f.proto, Proto::CurveTwocrypto | Proto::CurveTricrypto) {
        let Some(a) = reference.own_contract() else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на crypto-пул не того вида", f.title)));
        };
        return Ok(Loaded {
            state: load_crypto::load(ctx, f, a, pair, block).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Curve crypto v1 (решение 453): фабрика crypto v1 и tricrypto 2021 — свои геттеры, точность по decimals
    // (load_crypto_v1.rs).
    if matches!(f.proto, Proto::CurveCryptoV1 | Proto::CurveTricryptoV1) {
        let Some(a) = reference.own_contract() else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на crypto-пул не того вида", f.title)));
        };
        return Ok(Loaded {
            state: load_crypto_v1::load(ctx, tokens, f, a, pair, block).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Metric OMM (решение 285): вид ядра OraclePmm, пул — отдельный контракт, лесенка ответов пула (load_metric.rs).
    if f.proto == Proto::MetricOmm {
        if let PoolRef::Contract(a) = reference {
            return Ok(Loaded {
                state: load_metric::load(ctx, tokens, reg, f, *a, pair, usdt, &tag).await?,
                ticks: None,
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Metric не того вида", f.title),
        ));
    }
    // iZiSwap / Biswap V3 (решение 286): вид ядра OraclePmm, пул — отдельный контракт, лесенка ответов пула (load_izi.rs).
    if matches!(f.proto, Proto::IziSwap | Proto::BiswapV3) {
        if let PoolRef::Contract(a) = reference {
            return Ok(Loaded {
                state: load_izi::load(ctx, tokens, reg, f, *a, pair, usdt, &tag).await?,
                ticks: None,
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул iZiSwap не того вида", f.title),
        ));
    }
    // KyberSwap Classic DMM (решение 287): вид ядра AmpV2, пул — отдельный контракт, одно чтение getTradeInfo (load_dmm.rs).
    if f.proto == Proto::KyberDmm {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на пул DMM не того вида", f.title)));
        };
        return Ok(Loaded {
            state: load_dmm::load(ctx, *a, &tag).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Balancer V2 (решение 289): вид ядра VaultPool, пул — отдельный контракт, токены — в хранилище (load_balancer.rs).
    if f.proto == Proto::BalancerV2 {
        // Решение 295: и пара многомонетного пула (PoolRef::Multi).
        let Some(a) = reference.own_contract() else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на пул Balancer не того вида", f.title)));
        };
        return Ok(Loaded {
            state: load_balancer::load(ctx, f, a, pair, &tag).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Balancer V3 (решение 441): вид ядра VaultPool с частью V3, токены — в хранилище V3 (load_balancer_v3.rs).
    if f.proto == Proto::BalancerV3 {
        let Some(a) = reference.own_contract() else {
            let why = format!("{}: ссылка на пул Balancer V3 не того вида", f.title);
            return Err(ctx.err(ErrorCode::Internal, why));
        };
        return Ok(Loaded {
            state: load_balancer_v3::load(ctx, f, a, pair, &tag).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Liquidity Book V2.1 (решение 290): вид ядра LbBin, пул — отдельный контракт; состояние, время блока и полоса бинов
    // (load_lb.rs).
    if f.proto == Proto::LbV21 {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на пул LB не того вида", f.title)));
        };
        let (state, bins) = load_lb::load(ctx, tokens, *a, pair, usdt, block).await?;
        return Ok(Loaded { state, ticks: None, bins: Some(bins), fee_pips: None });
    }
    // Fluid DEX T1 (решение 440): вид ядра SmartColDebt, пул — отдельный контракт; резолвер (dex.quoter), слова пула и
    // время блока (load_fluid.rs).
    if f.proto == Proto::FluidT1 {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на пул Fluid не того вида", f.title)));
        };
        let resolver = ctx.dex_addr("dex.quoter", f.id)?;
        return Ok(Loaded {
            state: load_fluid::load(ctx, resolver, *a, block).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Camelot V2 (решение 443): вид ядра Solidly, пара — отдельный контракт; одно try_many (load_camelot.rs).
    if f.proto == Proto::CamelotV2 {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на пару Camelot не того вида", f.title)));
        };
        return Ok(Loaded {
            state: load_camelot::load(ctx, *a, &tag).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Liquidity Book v2.0 (решение 445): тот же вид ядра LbBin (признак v20), свои геттеры v2.0 (load_lb20.rs).
    if f.proto == Proto::LbV20 {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на пул LB v2.0 не того вида", f.title)));
        };
        let (state, bins) = load_lb20::load(ctx, tokens, *a, pair, usdt, block).await?;
        return Ok(Loaded { state, ticks: None, bins: Some(bins), fee_pips: None });
    }
    // SmarDex (решение 288): вид ядра Fictive, пара — отдельный контракт; состояние и время блока (load_smardex.rs).
    if f.proto == Proto::Smardex {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(ErrorCode::Internal, format!("{}: ссылка на пару SmarDex не того вида", f.title)));
        };
        return Ok(Loaded {
            state: load_smardex::load(ctx, *a, block).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Nomiswap Stable (решение 284): вид ядра StableSwap (шаблон input_fee), пара — отдельный контракт (load_nomi.rs).
    if f.proto == Proto::NomiStable {
        if let PoolRef::Contract(a) = reference {
            return Ok(Loaded {
                state: load_nomi::load(ctx, *a, pair, &tag).await?,
                ticks: None,
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пару Nomiswap Stable не того вида", f.title),
        ));
    }
    // PancakeSwap / Lista StableSwap, Saddle (Nerve) — решение 122 F, 3: вид ядра StableSwap, свои чтения
    // (load_stable_u.rs, load_saddle.rs). WooFi — пара общего контракта (PoolRef::Member), состояние оракула (load_woo.rs).
    let stable = matches!(
        f.proto,
        Proto::PancakeStable | Proto::ListaStable | Proto::SaddleStable | Proto::InfinityStable
    );
    if stable || f.proto == Proto::WooPP {
        let state = match (f.proto, reference) {
            // Решение 295: Saddle и PancakeSwap — и пара многомонетного пула (PoolRef::Multi); Lista — только Contract.
            (Proto::SaddleStable, PoolRef::Contract(a) | PoolRef::Multi { contract: a, .. }) => {
                load_saddle::load(ctx, tokens, *a, pair, &tag).await?
            }
            (Proto::PancakeStable, PoolRef::Multi { contract, .. }) => {
                load_stable_u::load(ctx, tokens, *contract, pair, false, &tag).await?
            }
            (Proto::PancakeStable | Proto::ListaStable, PoolRef::Contract(a)) => {
                load_stable_u::load(ctx, tokens, *a, pair, f.proto == Proto::ListaStable, &tag)
                    .await?
            }
            // Infinity Stable (решение «122 (Infinity Stable)»): состояние у крючка (load_inf_stable.rs).
            (Proto::InfinityStable, PoolRef::Contract(a)) => {
                load_inf_stable::load(ctx, f, *a, pair, &tag).await?
            }
            (Proto::WooPP, PoolRef::Member { contract, coin }) => {
                load_woo::load(ctx, tokens, reg, f, *contract, *coin, pair, &tag).await?
            }
            _ => {
                return Err(ctx.err(
                    ErrorCode::Internal,
                    format!("{}: ссылка на пул не того вида", f.title),
                ));
            }
        };
        return Ok(Loaded {
            state,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Maverick V2 (решение 122 F): вид ядра TickBin, чтение своё (load_maverick.rs): состояние, тики и кривая квотера.
    if f.proto == Proto::Maverick {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пул Maverick не того вида", f.title),
            ));
        };
        let state = load_maverick::load(ctx, f, *a, pair, &tag).await?;
        return Ok(Loaded {
            state,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // KyberSwap Elastic (решение 81): вид ядра Cl, но пул — отдельный контракт, чтение своё (load_elastic.rs).
    if f.proto == Proto::KyberElastic {
        if let PoolRef::Contract(a) = reference {
            let sp = spacing.ok_or_else(|| missing("tickDistance"))?;
            let (state, t) = load_elastic::load(ctx, f, *a, sp, &tag).await?;
            return Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Elastic не того вида", f.title),
        ));
    }
    // Algebra (THENA Fusion, решения 80, 81): вид ядра Cl, но пул — отдельный контракт, чтение своё (load_algebra.rs):
    // globalState + liquidity + сырой битмап tickTable. tickSpacing пулу не нужен (битмап индексирует сырой тик).
    if matches!(f.proto, Proto::Algebra | Proto::AlgebraIntegral | Proto::AlgebraDirFee) {
        if let PoolRef::Contract(a) = reference {
            // Решение 188: у Integral тики — связным списком (load_algebra_integral.rs), состояние то же. Решение 444:
            // у направленных комиссий — свой globalState (load_algebra_dir.rs), карта — та же, что у V1.9.
            let (state, t) = if f.proto == Proto::AlgebraIntegral {
                load_algebra_integral::load(ctx, *a, &tag).await?
            } else if f.proto == Proto::AlgebraDirFee {
                load_algebra_dir::load(ctx, *a, &tag).await?
            } else {
                load_algebra::load(ctx, *a, &tag).await?
            };
            return Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Algebra не того вида", f.title),
        ));
    }
    // Solidly (Topaz v2, THENA V1; решения 80, 85): вид ядра Solidly, пул — отдельный контракт, чтение своё
    // (load_solidly.rs): metadata() пула (резервы, decimals, stable) + getFee(pool, stable) фабрики. Карт тиков/бинов
    // нет. Нужен id DEX (адрес фабрики для getFee).
    if f.proto.is_solidly() {
        if let PoolRef::Contract(a) = reference {
            return Ok(Loaded {
                state: load_solidly::load(ctx, f, *a, &tag).await?,
                ticks: None,
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Solidly не того вида", f.title),
        ));
    }
    // Slipstream (Topaz v3; решения 80, 85): вид ядра V3, пул — отдельный контракт, НО чтение своё (load_slipstream.rs):
    // slot0 форка (без feeProtocol) + liquidity + тики прямыми геттерами tickBitmap/ticks (не TickLens). Комиссия
    // (fee_pips) уже прочитана с fee() пула при проверке (verify.rs) и хранится в записи. Отсекается до общего пути V3
    // (который читал бы через TickLens и чужой slot0).
    if f.proto == Proto::Slipstream {
        if let PoolRef::Contract(a) = reference {
            let sp = spacing.ok_or_else(|| missing("tickSpacing"))?;
            let (state, t) = load_slipstream::load(ctx, *a, sp, &tag).await?;
            return Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
                fee_pips: None,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Slipstream не того вида", f.title),
        ));
    }
    match (f.kind, reference) {
        (PoolKind::V2, PoolRef::Contract(a)) => Ok(Loaded {
            state: load_v2v3::v2(ctx, *a, &tag).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        }),
        (PoolKind::V3, PoolRef::Contract(a)) => {
            let sp = spacing.ok_or_else(|| missing("tickSpacing"))?;
            let (state, t) = load_v2v3::v3(ctx, f, *a, sp, &tag).await?;
            // Решение 281: свежая fee() пула на том же блоке — только у DEX с dex.v3_fee_source = pool_fee_each_read.
            let fee_pips = if fee_fresh::on(ctx, f) {
                Some(fee_fresh::read(ctx, *a, &tag).await?)
            } else {
                None
            };
            Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
                fee_pips,
            })
        }
        (PoolKind::Cl, PoolRef::Singleton { manager, id }) => {
            let sp = spacing.ok_or_else(|| missing("tickSpacing"))?;
            let (state, t) = load_cl::load(ctx, f, *manager, *id, sp, &tag).await?;
            Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
                fee_pips: None,
            })
        }
        (PoolKind::Bin, PoolRef::Singleton { manager, id }) => {
            let st = step.ok_or_else(|| missing("binStep"))?;
            let depth = load_bin::Depth {
                usdt_is_y: usdt == token1,
                usdt_decimals: tokens.decimals(ctx, usdt).await?,
            };
            let (state, b) = load_bin::load(ctx, *manager, *id, st, &depth, &tag).await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: Some(b),
                fee_pips: None,
            })
        }
        _ => Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул не того вида", f.title),
        )),
    }
}

/// Запись реестра из найденного пула: читает состояние на блоке.
/// Ошибки: как у state.
pub async fn entry(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    p: &Found,
    ticker: &str,
    usdt: Address,
    block: u64,
) -> GatewayResult<Entry> {
    let l = state(
        ctx,
        tokens,
        reg,
        p.family,
        &p.reference,
        p.tick_spacing,
        p.bin_step,
        usdt,
        (p.token0, p.token1),
        block,
    )
    .await?;
    Ok(built(ctx, p, ticker, usdt, block, l))
}

/// Запись реестра из найденного пула и уже прочитанного состояния (и пачечное чтение, load_batch.rs). Ошибок нет.
pub fn built(ctx: &Ctx, p: &Found, ticker: &str, usdt: Address, block: u64, l: Loaded) -> Entry {
    // Свежая fee() пула (решение 281) — вместо комиссии из проверки; без неё — как раньше.
    let fee_pips = l.fee_pips.or(p.fee_pips.filter(|f| *f != DYNAMIC_FEE_FLAG));
    let pool = Pool {
        venue: ctx.venue(),
        family: p.family.id.to_string(),
        kind: p.family.kind,
        reference: p.reference.clone(),
        token0: p.token0,
        token1: p.token1,
        fee_pips,
        tick_spacing: p.tick_spacing,
        bin_step: p.bin_step,
        hooks: p.hooks.clone(),
        state: l.state,
        block,
        received_at: Utc::now(),
    };
    Entry {
        family: p.family,
        pool,
        ticker: ticker.to_string(),
        coin: p.coin,
        usdt,
        key: p.key.clone(),
        ticks: l.ticks,
        bins: l.bins,
        synced_block: block,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}

/// Перечитывает состояние записи на блоке. Принимает: окружение, токены, реестр (как у state), запись.
/// Возвращает: обновлённую запись (valid = true).
/// Ошибки: как у state.
pub async fn refresh(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    e: &Entry,
    block: u64,
) -> GatewayResult<Entry> {
    let p = &e.pool;
    let l = state(
        ctx,
        tokens,
        reg,
        e.family,
        &p.reference,
        p.tick_spacing,
        p.bin_step,
        e.usdt,
        (p.token0, p.token1),
        block,
    )
    .await?;
    Ok(renewed(e, block, l))
}

/// Запись с новым состоянием на блоке (valid = true). Ошибок нет.
pub fn renewed(e: &Entry, block: u64, l: Loaded) -> Entry {
    let mut out = e.clone();
    out.pool.state = l.state;
    // Свежая fee() пула на этом блоке (решение 281); None — комиссия записи прежняя.
    if let Some(fee) = l.fee_pips {
        out.pool.fee_pips = Some(fee);
    }
    out.pool.block = block;
    out.pool.received_at = Utc::now();
    out.ticks = l.ticks;
    out.bins = l.bins;
    out.synced_block = block;
    out.synced_log = None;
    out.valid = true;
    // Решение 301 (е2): запись вне реестра (свежее состояние) — поколение присвоит put; до него глубина считается заново.
    out.state_gen = 0;
    out
}
