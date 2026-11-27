//! Файл: venues/bsc_dex/pools/load.rs
//! Что делает: чтение состояния любого пула на заданном блоке (по его виду) и сборка записи реестра
//! из найденного пула; перечитывание записи (после реорга, обрыва или расхождения карты с событием).
//! Кто вызывает: pools/listen.rs.
//! От чего зависит: load_v2v3.rs, load_cl.rs, load_bin.rs, load_lb.rs, load_lb20.rs, load_fluid.rs, load_ekubo.rs
//! (решение 447), load_ramses.rs (решение 483), registry.rs, discover.rs, tokens.rs.
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
use super::family_variant::Variant;
use super::fee_fresh;
use super::hooks::DYNAMIC_FEE_FLAG;
use super::load_algebra_dir;
use super::registry::{Entry, Registry};
use super::ticks::TickMap;
use super::{
    aero_cl, load_aero_basic, load_algebra, load_algebra_integral, load_balancer, load_balancer_v3,
    load_basin, load_bin, load_camelot, load_cl, load_crypto, load_crypto_v1, load_curve,
    load_curve_meta, load_curve_ng, load_curve_oracle, load_curve_plain, load_dmm, load_dodo,
    load_ekubo, load_elastic, load_fluid, load_inf_stable, load_izi, load_lb, load_lb20,
    load_maverick, load_metric, load_nomi, load_ramses, load_saddle, load_slipstream, load_smardex,
    load_solidly, load_stable_u, load_v2v3, load_wombat, load_woo,
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
// Один запрос состояния владеет своим future, независимо от размера самого крупного
// загрузчика. Пакет Multicall остальных пулов не выделяет память для этих чтений.
#[allow(clippy::too_many_arguments)]
pub fn state<'a>(
    ctx: &'a Ctx,
    tokens: &'a Tokens,
    reg: &'a Registry,
    f: &'a Family,
    reference: &'a PoolRef,
    spacing: Option<i32>,
    step: Option<u32>,
    usdt: Address,
    pair: (Address, Address),
    block: u64,
) -> futures_util::future::BoxFuture<'a, GatewayResult<Loaded>> {
    // Construct and poll only this family's fallible RPC sequence. In debug builds
    // unused arms of one async function otherwise consume real stack in poll().
    if f.variant.launch()
        && let PoolRef::Singleton { manager, id } = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            Box::pin(super::launch_ladder::load(
                ctx, tokens, reg, f, *manager, *id, pair, &tag,
            ))
            .await
        });
    }
    if f.variant == Variant::DeliCl
        && let PoolRef::Singleton { manager, id } = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            Box::pin(super::deli_cl::load(ctx, f, *manager, *id, pair, &tag)).await
        });
    }
    if f.variant == Variant::DeliConstant
        && let PoolRef::Singleton { manager, id } = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let (key, reserve0, reserve1) =
                Box::pin(super::deli::market(ctx, f, *manager, *id, pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::V2 { reserve0, reserve1 },
                ticks: None,
                bins: None,
                fee_pips: Some(key.fee()),
            })
        });
    }
    if f.variant == Variant::Stabull {
        return Box::pin(async move {
            let tag = block_tag(block);
            let pool = reference.address();
            let at = super::nami::block(ctx, &tag).await?;
            Box::pin(super::stabull::market(ctx, f, pool, pair, &at)).await?;
            let state = Box::pin(super::stabull_quotes::ladder(
                ctx, tokens, reg, f, pool, pair, usdt, &at,
            ))
            .await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::MaverickOne {
        return Box::pin(async move {
            let tag = block_tag(block);
            let pool = reference.address();
            let at = super::nami::block(ctx, &tag).await?;
            Box::pin(super::maverick_one::market(ctx, f, pool, pair, &at)).await?;
            let state = Box::pin(super::maverick_one_quotes::ladder(
                ctx,
                tokens,
                reg,
                f,
                pool,
                [pair.0, pair.1],
                usdt,
                &at,
            ))
            .await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant.umbrae() {
        return Box::pin(async move {
            let tag = block_tag(block);
            let pool = reference.address();
            let at = super::nami::block(ctx, &tag).await?;
            let m = Box::pin(super::umbrae::market(ctx, f, pool, pair, &at)).await?;
            let state = if f.variant == Variant::UmbraeDamm {
                PoolState::UmbraeDamm(Box::new(
                    Box::pin(super::umbrae_damm_state::read(ctx, pool, m.tokens, &at)).await?,
                ))
            } else {
                Box::pin(super::umbrae_quotes::ladder(
                    ctx, tokens, reg, f, pool, m.tokens, usdt, &at,
                ))
                .await?
            };
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::Bundles {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let book =
                Box::pin(super::bundles::book(ctx, f, reference.address(), pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::Bundles(Box::new(book)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::Antfarm {
        return Box::pin(async move {
            let tag = block_tag(block);
            let h = super::super::quote::white_state::head(ctx, &tag).await?;
            let mut s = Box::pin(super::antfarm_book::read(
                ctx,
                f,
                reference.address(),
                pair,
                h,
            ))
            .await?;
            s.limits = Box::pin(super::antfarm_capacity::available(ctx, f, &s, h)).await;
            Ok(Loaded {
                state: PoolState::Antfarm(Box::new(s)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::Surge {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state =
                Box::pin(super::surge::market(ctx, f, reference.address(), pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::Surge(Box::new(state)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::CypherIntegral {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::cypher_quotes::load(
                ctx,
                tokens,
                reg,
                f,
                reference.address(),
                pair,
                usdt,
                &at,
            ))
            .await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant.protected_router_wallet() {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::safeswap_quotes::load(
                ctx,
                tokens,
                reg,
                f,
                reference.address(),
                pair,
                usdt,
                &at,
            ))
            .await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::BalancerV1 {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let s = Box::pin(super::balancer_v1::load(ctx, f, reference.address(), &at)).await?;
            if ![pair.0, pair.1]
                .iter()
                .all(|a| s.assets.iter().any(|v| v.token == *a))
            {
                return Err(super::balancer_v1::bad(ctx));
            }
            Ok(Loaded {
                state: PoolState::BalancerV1(Box::new(s)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::BancorV3 {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let b = Box::pin(super::bancor_v3::load(
                ctx,
                f,
                reference,
                [pair.0, pair.1],
                &at,
            ))
            .await?;
            Ok(Loaded {
                state: PoolState::BancorV3(Box::new(b)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::UniswapV1 {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let b = Box::pin(super::uniswap_v1::load(
                ctx,
                f,
                reference,
                [pair.0, pair.1],
                &at,
            ))
            .await?;
            Ok(Loaded {
                state: PoolState::UniswapV1(Box::new(b)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::RubiconClassic {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let b = Box::pin(super::rubicon::load(ctx, f, reference.address(), pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::Rubicon(Box::new(b)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::PropVenue {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::prop_quotes::load(
                ctx,
                tokens,
                reg,
                f,
                reference.address(),
                pair,
                usdt,
                &at,
            ))
            .await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::IntegralSize {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::integral_quotes::load(
                ctx,
                tokens,
                reg,
                f,
                reference.address(),
                pair,
                usdt,
                &at,
            ))
            .await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::Pendle {
        return Box::pin(async move {
            let tag = block_tag(block);
            let h = super::super::quote::white_state::head(ctx, &tag).await?;
            let state = Box::pin(super::pendle_quotes::load(
                ctx,
                tokens,
                reg,
                f,
                reference.address(),
                pair,
                usdt,
                h,
            ))
            .await?;
            Ok(Loaded {
                state,
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::Dagrow {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::dagrow::state(
                ctx,
                f,
                reference.address(),
                Some(pair),
                &at,
            ))
            .await?;
            Ok(Loaded {
                state: PoolState::Dagrow(Box::new(state)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::Temple {
        return Box::pin(async move {
            let tag = block_tag(block);
            let PoolRef::Multi { contract, .. } = reference else {
                return Err(ctx.err(
                    crate::gateway::errors::ErrorCode::NoRoute,
                    "TempleDAO: нужна пара с указанным активом выплаты",
                ));
            };
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::temple::market(ctx, f, *contract, pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::Temple(Box::new(state)),
                ticks: None,
                bins: None,
                fee_pips: Some(5000),
            })
        });
    }
    if f.variant == Variant::Carbon
        && let PoolRef::Singleton { manager, id } = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::carbon::market(ctx, f, *manager, *id, pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::Carbon(Box::new(state)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::MintClub
        && let PoolRef::Contract(token) = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::mint_club::market(ctx, f, *token, pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::MintClub(Box::new(state)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::Virtuals
        && let PoolRef::Contract(pool) = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let state = Box::pin(super::virtuals::market(ctx, f, *pool, pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::Virtuals(Box::new(state)),
                ticks: None,
                bins: None,
                fee_pips: None,
            })
        });
    }
    if f.variant == Variant::Gemswap
        && let PoolRef::Contract(pool) = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let m = Box::pin(super::concave::market(ctx, f, *pool, pair, &at)).await?;
            Ok(Loaded {
                state: PoolState::V2 {
                    reserve0: m.reserves.0,
                    reserve1: m.reserves.1,
                },
                ticks: None,
                bins: None,
                fee_pips: Some(m.fee),
            })
        });
    }
    if f.variant == Variant::ToonSwap
        && let PoolRef::Contract(pool) = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            Box::pin(super::toonswap::read(ctx, f, *pool, pair, &at)).await
        });
    }
    if f.variant == Variant::BunnySwap
        && let PoolRef::Contract(pool) = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            let at = super::nami::block(ctx, &tag).await?;
            let fee = Box::pin(super::bunnyswap::market(ctx, f, *pool, pair, &at)).await?;
            let get = super::abi_v2v3::IV2Pair::getReservesCall {};
            let [r] =
                super::nami_batch::read(ctx, [super::nami_batch::view(&at, *pool, get.clone())])
                    .await?;
            let r = super::nami_batch::decode(&r, get)?;
            Ok(Loaded {
                state: PoolState::V2 {
                    reserve0: alloy_primitives::U256::from(r.reserve0),
                    reserve1: alloy_primitives::U256::from(r.reserve1),
                },
                ticks: None,
                bins: None,
                fee_pips: Some(fee),
            })
        });
    }
    if f.variant == Variant::Memebox
        && let PoolRef::Contract(pool) = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            Box::pin(super::memebox::load(ctx, f, *pool, pair, &tag)).await
        });
    }
    if f.variant == Variant::WhiteSwap
        && let PoolRef::Contract(pool) = reference
    {
        return Box::pin(async move {
            let tag = block_tag(block);
            let hash = super::elfomo::block(ctx, &tag).await?;
            let (reserve0, reserve1) =
                Box::pin(super::whiteswap::check(ctx, f, *pool, pair, &hash)).await?;
            Ok(Loaded {
                state: PoolState::V2 { reserve0, reserve1 },
                ticks: None,
                bins: None,
                fee_pips: Some(3_000),
            })
        });
    }
    Box::pin(read(
        ctx, tokens, reg, f, reference, spacing, step, usdt, pair, block,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn read(
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
    if f.variant == Variant::Spectra {
        Box::pin(super::spectra::check(ctx, f, reference.address(), &tag)).await?;
    }
    if f.variant.registered_v3() && let PoolRef::Contract(pool)=reference {
        let at=super::nami::block(ctx,&tag).await?;
        Box::pin(super::registered_v3::market(ctx,f,*pool,pair,None,&at)).await?;
    }
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
            // Решение 472: вариант Curve 2021 (curve_stableswap_plain) — A_precise() и свой шаблон (load_curve_plain.rs).
            let state = match f.variant {
                Variant::CurvePlain => load_curve_plain::load(ctx, tokens, a, pair, &tag).await?,
                // Решение 472 (доделка): метапул — курс LP базового пула (load_curve_meta.rs).
                Variant::CurveMeta => load_curve_meta::load(ctx, tokens, a, pair, &tag).await?,
                // Решение 481: пул с курсом оракула и нативным ETH (ETH/wstETH, load_curve_oracle.rs).
                Variant::CurveOracle => load_curve_oracle::load(ctx, tokens, a, pair, &tag).await?,
                _ => load_curve::load(ctx, tokens, a, pair, &tag).await?,
            };
            return Ok(Loaded {
                state,
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
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на crypto-пул не того вида", f.title),
            ));
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
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на crypto-пул не того вида", f.title),
            ));
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
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пул DMM не того вида", f.title),
            ));
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
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пул Balancer не того вида", f.title),
            ));
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
    // Решение 446 (ревью С1, С3): Liquidity Book V2.2 — то же чтение и в той же пачке слово крючков (lb_hooks.rs load);
    // недопущенный крючок обмена — не ошибка чтения: состояние пишется, обмен через пул исключён с причиной.
    if matches!(f.proto, Proto::LbV21 | Proto::LbV22) {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пул LB не того вида", f.title),
            ));
        };
        let (state, bins) = if f.proto == Proto::LbV22 {
            super::lb_hooks::load(ctx, f, tokens, *a, pair, usdt, block).await?
        } else {
            load_lb::load(ctx, tokens, *a, pair, usdt, block).await?
        };
        return Ok(Loaded {
            state,
            ticks: None,
            bins: Some(bins),
            fee_pips: None,
        });
    }
    // Fluid DEX T1 (решение 440): вид ядра SmartColDebt, пул — отдельный контракт; резолвер (dex.quoter), слова пула и
    // время блока (load_fluid.rs).
    if f.proto == Proto::FluidT1 {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пул Fluid не того вида", f.title),
            ));
        };
        let resolver = ctx.dex_addr("dex.quoter", f.id)?;
        return Ok(Loaded {
            state: load_fluid::load(ctx, resolver, *a, block).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Скважина Basin (решение 485): вид ядра BasinWell, скважина — отдельный контракт; слот ERC-1967 и одно try_many
    // (load_basin.rs).
    if f.proto == Proto::BasinWell {
        let PoolRef::Contract(a) = reference else {
            let why = format!("{}: ссылка на скважину Basin не того вида", f.title);
            return Err(ctx.err(ErrorCode::Internal, why));
        };
        return Ok(Loaded {
            state: load_basin::load(ctx, f.id, *a, block).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Shell Protocol Proteus (решение 486): вид ядра Proteus — поля пула, балансы в учёте Ocean, параметры
    // реализации и знаки токенов (load_shell.rs).
    if f.proto == Proto::ShellProteus {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пул Shell не того вида", f.title),
            ));
        };
        return Ok(Loaded {
            state: super::load_shell::load(ctx, f.id, *a, pair, block).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Camelot V2 (решение 443): вид ядра Solidly, пара — отдельный контракт; одно try_many (load_camelot.rs).
    if f.proto == Proto::CamelotV2 {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пару Camelot не того вида", f.title),
            ));
        };
        return Ok(Loaded {
            state: load_camelot::load(ctx, *a, &tag).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Решение 470: Aero Basic — вид ядра Solidly, пул — отдельный контракт; metadata/POOL_TYPE пула и комиссия,
    // пауза, модуль MEV-налога фабрики (load_aero_basic.rs).
    if f.proto == Proto::AeroBasic {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пул Aero Basic не того вида", f.title),
            ));
        };
        return Ok(Loaded {
            state: load_aero_basic::load(ctx, f, *a, &tag).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // Liquidity Book v2.0 (решение 445): тот же вид ядра LbBin (признак v20), свои геттеры v2.0 (load_lb20.rs).
    if f.proto == Proto::LbV20 {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пул LB v2.0 не того вида", f.title),
            ));
        };
        let (state, bins) = load_lb20::load(ctx, tokens, *a, pair, usdt, block).await?;
        return Ok(Loaded {
            state,
            ticks: None,
            bins: Some(bins),
            fee_pips: None,
        });
    }
    // Ekubo (решение 447): вид ядра Ekubo, пул — в ядре (PoolId); состояние, битмап, тики, MEVCapture и время блока
    // (load_ekubo.rs).
    if f.proto == Proto::Ekubo {
        let PoolRef::Singleton { manager, id } = reference else {
            let why = format!("{}: ссылка на пул Ekubo не того вида", f.title);
            return Err(ctx.err(ErrorCode::Internal, why));
        };
        return Ok(Loaded {
            state: load_ekubo::load(ctx, f, *manager, *id, block).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // SmarDex (решение 288): вид ядра Fictive, пара — отдельный контракт; состояние и время блока (load_smardex.rs).
    if f.proto == Proto::Smardex {
        let PoolRef::Contract(a) = reference else {
            return Err(ctx.err(
                ErrorCode::Internal,
                format!("{}: ссылка на пару SmarDex не того вида", f.title),
            ));
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
    // Решение 484: Wombat — вид ядра StableSwap (часть StableData.wombat), чтение — load_wombat.rs (и пара многомонетного
    // пула — PoolRef::Multi, когда пара читается одна).
    if f.proto == Proto::Wombat {
        let a = match reference {
            PoolRef::Contract(a) | PoolRef::Multi { contract: a, .. } => a,
            _ => {
                let why = format!("{}: ссылка на пул Wombat не того вида", f.title);
                return Err(ctx.err(ErrorCode::Internal, why));
            }
        };
        return Ok(Loaded {
            state: load_wombat::load(ctx, f, *a, pair, &tag).await?,
            ticks: None,
            bins: None,
            fee_pips: None,
        });
    }
    // PancakeSwap / Lista StableSwap, Saddle (Nerve) — решение 122 F, 3: вид ядра StableSwap, свои чтения
    // (load_stable_u.rs, load_saddle.rs). WooFi — пара общего контракта (PoolRef::Member), состояние оракула (load_woo.rs).
    let stable = matches!(
        f.proto,
        Proto::PancakeStable | Proto::ListaStable | Proto::SaddleStable | Proto::InfinityStable
    );
    if stable
        || matches!(
            f.proto,
            Proto::WooPP | Proto::Elfomo | Proto::Lunar | Proto::BaiBai
        )
    {
        let state = match (f.proto, reference) {
            // Решение 295: Saddle и PancakeSwap — и пара многомонетного пула (PoolRef::Multi); Lista — только Contract.
            (Proto::SaddleStable, PoolRef::Contract(a) | PoolRef::Multi { contract: a, .. }) => {
                load_saddle::load(ctx, tokens, *a, pair, &tag).await?
            }
            (Proto::PancakeStable, PoolRef::Multi { contract, .. }) => {
                load_stable_u::load(ctx, tokens, *contract, pair, false, &tag).await?
            }
            (Proto::PancakeStable | Proto::ListaStable, PoolRef::Contract(a)) => {
                if f.variant == super::family_variant::Variant::ListaStableProxy {
                    load_stable_u::check_implementation(ctx, f.id, *a, &tag).await?;
                }
                load_stable_u::load(ctx, tokens, *a, pair, f.proto == Proto::ListaStable, &tag)
                    .await?
            }
            // Infinity Stable (решение «122 (Infinity Stable)»): состояние у крючка (load_inf_stable.rs).
            (Proto::InfinityStable, PoolRef::Contract(a)) => {
                load_inf_stable::load(ctx, f, *a, pair, &tag).await?
            }
            (Proto::BaiBai, PoolRef::Member { contract, coin })
                if pair.0 == *coin || pair.1 == *coin =>
            {
                super::baibai::load(ctx, tokens, reg, f, *contract, pair, &tag).await?
            }
            (Proto::BaiBai, PoolRef::Contract(pool))
                if super::stable_native::wrapped(ctx)
                    .is_some_and(|base| base == pair.0 || base == pair.1) =>
            {
                super::baibai::load(ctx, tokens, reg, f, *pool, pair, &tag).await?
            }
            (Proto::Lunar, PoolRef::Contract(pool)) => {
                super::lunar::load(ctx, tokens, reg, f, *pool, pair, &tag).await?
            }
            (Proto::Elfomo, PoolRef::Member { contract, coin }) => {
                super::elfomo::load(ctx, tokens, reg, f, *contract, *coin, pair, &tag).await?
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
            let at = if f.variant == Variant::ElasticV5 {
                let at = super::elfomo::block(ctx,&tag).await?;
                Box::pin(super::elastic_binding::check(ctx,f,*a,&at)).await?;
                Some(at)
            } else { None };
            let (state, t) = load_elastic::load(ctx, f, *a, sp, &tag).await?;
            if let Some(at)=at && super::elfomo::block(ctx,&tag).await? != at {
                return Err(ctx.err(ErrorCode::StaleData,"Elastic: блок изменился при чтении тиков"));
            }
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
    // globalState + liquidity + tickSpacing + сжатый битмап tickTable (решение 463: код AlgebraV1, строка —
    // 256·spacing); у Algebra V1.9 с направленными комиссиями (решение 444) — сырой битмап, tickSpacing не нужен.
    if matches!(
        f.proto,
        Proto::Algebra | Proto::AlgebraIntegral | Proto::AlgebraDirFee
    ) {
        if let PoolRef::Contract(a) = reference {
            // Решение 188: у Integral тики — связным списком (load_algebra_integral.rs), состояние то же. Решение 444:
            // у направленных комиссий — свой globalState (load_algebra_dir.rs), карта — та же, что у V1.9.
            let (state, t) = if f.variant == Variant::SupernovaIntegral {
                let hash=super::elfomo::block(ctx,&tag).await?;
                let (fee,flags)=Box::pin(super::supernova::check(ctx,f,*a,&hash)).await?;
                Box::pin(load_algebra_integral::canonical(ctx,*a,&hash,fee,flags)).await?
            } else if f.variant == Variant::NamiIntegral {
                let hash = super::elfomo::block(ctx, &tag).await?;
                let fee = Box::pin(super::nami::check(ctx, f.id, *a, &hash)).await?;
                Box::pin(load_algebra_integral::canonical(ctx, *a, &hash, fee, super::nami::PLUGIN_FLAGS)).await?
            } else if matches!(f.variant,Variant::BasePluginV1|Variant::TrebleIntegral) {
                let hash=super::elfomo::block(ctx,&tag).await?;
                let fee=Box::pin(super::base_plugin_v1::check(ctx,f,*a,&hash)).await?;
                Box::pin(load_algebra_integral::canonical(ctx,*a,&hash,fee,super::base_plugin_v1::flags(f.variant))).await?
            } else if f.proto == Proto::AlgebraIntegral {
                if f.variant == Variant::QuickswapIntegral {
                    super::quickswap_plugin::check(ctx, f.id, *a, &tag).await?;
                }
                load_algebra_integral::load(ctx, *a, &tag).await?
            } else if f.proto == Proto::AlgebraDirFee {
                load_algebra_dir::load(ctx, *a, &tag).await?
            } else {
                let step=super::synth_farming::step(ctx,f)?;
                if step.is_some() {
                    let at=super::elfomo::block(ctx,&tag).await?;
                    Box::pin(super::synth_farming::check(ctx,f,*a,&at)).await?;
                    let loaded=load_algebra::load(ctx,*a,Some(1),&tag,step).await?;
                    if super::elfomo::block(ctx,&tag).await?!=at {return Err(ctx.err(ErrorCode::StaleData,"Algebra V1.9: блок заменён"));}
                    loaded
                }else{load_algebra::load(ctx, *a, spacing, &tag,None).await?}
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
    // Решение 483: Ramses CL / CL V2 — свои ABI slot0 / ticks() и комиссия этого блока (currentFee() / fee()) при
    // каждом чтении (load_ramses.rs). Отсекается до Slipstream и общего пути V3 (ticks() из 10 полей — не через TickLens).
    if super::ramses::is_cl(f) {
        if let PoolRef::Contract(a) = reference {
            let sp = spacing.ok_or_else(|| missing("tickSpacing"))?;
            let (state, t, fee) = load_ramses::load(ctx, f, *a, sp, &tag).await?;
            return Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
                fee_pips: Some(fee),
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Ramses CL не того вида", f.title),
        ));
    }
    if f.proto == Proto::Slipstream {
        if let PoolRef::Contract(a) = reference {
            let sp = spacing.ok_or_else(|| missing("tickSpacing"))?;
            if f.variant == Variant::SolidlyCl {
                if f.fork {
                    super::direct_module::verify(ctx, f, ctx.dex_addr("dex.router", f.id)?, &tag)
                        .await?;
                }
                let (state, ticks, fee) = super::solidly_v3::load(ctx, *a, sp, &tag).await?;
                return Ok(Loaded {
                    state,
                    ticks: Some(ticks),
                    bins: None,
                    fee_pips: Some(fee),
                });
            }
            let (state, t) = load_slipstream::load(ctx, *a, sp, &tag).await?;
            // Решение 470: Aero Slipstream V3 — комиссия крючка на этом блоке и проверки (aero_cl.rs).
            let fee_pips = match f.variant {
                Variant::SlipstreamAero | Variant::SlipstreamAeroMetaquoter => {
                    Some(aero_cl::fee(ctx, f, *a, sp, block).await?)
                }
                // CLPool.fee() делегирует фабрике getSwapFee(pool), в том числе модулю комиссии.
                // https://github.com/aerodrome-finance/slipstream/blob/main/contracts/core/CLPool.sol
                Variant::TopazCl => {let at=super::nami::block(ctx,&tag).await?;Some(Box::pin(super::topaz::fee(ctx,f,*a,pair,sp,&at)).await?)},
                Variant::SlipstreamDynamicFee | Variant::SlipstreamUniversal => Some(fee_fresh::read(ctx, *a, &tag).await?),
                _ => None,
            };
            return Ok(Loaded {
                state,
                ticks: Some(t),
                bins: None,
                fee_pips,
            });
        }
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("{}: ссылка на пул Slipstream не того вида", f.title),
        ));
    }
    match (f.kind, reference) {
        (PoolKind::V2, PoolRef::Contract(a)) if f.variant == Variant::EtherVista => {
            Box::pin(super::ethervista::load(ctx,f,*a,pair,&tag)).await
        }
        (PoolKind::V2, PoolRef::Contract(a)) if f.variant == Variant::Oasis => {
            Box::pin(super::oasis::load(ctx,f,*a,pair,&tag)).await
        }
        (PoolKind::V2, PoolRef::Contract(a)) if f.variant == Variant::DeltaSwap => {
            Box::pin(super::deltaswap::load(ctx, f, *a, pair, block)).await
        }
        (PoolKind::V2, PoolRef::Contract(a)) if f.variant.frax_twamm() => {
            Box::pin(super::fraxswap::load(ctx, f, *a, pair, &tag)).await
        }
        (PoolKind::V2, PoolRef::Contract(a)) if f.variant.live_v2_fee() => {
            super::v2_live_fee::load(
                ctx,
                f,
                super::v2_live_fee::Market {
                    pool: *a,
                    tokens: pair,
                },
                &tag,
            )
            .await
        }
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
    let tick_spacing = known_spacing(p.family, p.tick_spacing, &l);
    let pool = Pool {
        venue: ctx.venue(),
        family: p.family.id.to_string(),
        kind: p.family.kind,
        reference: p.reference.clone(),
        token0: p.token0,
        token1: p.token1,
        fee_pips,
        tick_spacing,
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

/// Ревью t236 (решение 463): tickSpacing пула THENA Fusion неизменен (Constants.TICK_SPACING кода пула) — после
/// первого чтения (load_algebra.rs) он хранится в Pool.tick_spacing записи, и перечитывание его не запрашивает. Другие
/// виды — прежнее значение. Принимает: вид, прежний tickSpacing, прочитанное. Возвращает: tickSpacing записи. Ошибок нет.
fn known_spacing(f: &Family, old: Option<i32>, l: &Loaded) -> Option<i32> {
    if f.proto != Proto::Algebra || f.variant==Variant::AlgebraSingleV19 {
        return old;
    }
    let map = l.ticks.as_ref().map(|m| m.spacing);
    old.or(map)
}

/// Запись с новым состоянием на блоке (valid = true). Ошибок нет.
pub fn renewed(e: &Entry, block: u64, l: Loaded) -> Entry {
    let mut out = e.clone();
    out.pool.tick_spacing = known_spacing(e.family, e.pool.tick_spacing, &l);
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
