//! Файл: venues/bsc_dex/pools/factory_calls.rs
//! Что делает: вызовы реестра фабрик для поиска пулов на своём узле (решение 121, C3): по каждой подключённой DEX
//! вида V2 / форк V2 — getPair(монета, встречный токен); V3 (PancakeSwap / Uniswap ABI) — getPool(монета, встречный,
//! fee) по каждому уровню комиссии из dex.v3_fee_tiers этой DEX (не задана — V3 этой DEX по фабрике не ищется).
//! Решение 122 F: ещё Solidly / Solidly V1 (по обоим stable), Algebra (poolByPair) и DODO V2 (getDODOPoolBidirection —
//! индексатор пулы DODO на BSC не отдаёт); вид вопроса и разбор — factory_ask.rs. Slipstream — getPool(a, b, int24
//! tickSpacing) по каждому уровню из dex.cl_tick_spacings (не задана — Slipstream этой DEX по фабрике не ищется).
//! DEX не подключена (families::classify) — не ищется, причина в заметках прохода. Только сборка вызовов и разбор
//! ответа — без сети.
//! Кто вызывает: pools/factory_scan.rs.
//! От чего зависит: families.rs (all, classify, Proto), abi_v2v3.rs (IV2Factory, IV3Factory), ctx.rs, multi_pair.rs
//! (woo_pairs), woo_quote.rs (quote_of).
//! Решение 450 (T448-05): iZiSwap — pool(монета, встречный, fee) по уровням dex.v3_fee_tiers (iZiSwap нет у
//! индексатора в сети arbitrum — пулы только поиском по фабрике).
//! Решение 475 (T462-01): KyberSwap Elastic (kyber_elastic, kyber_elastic_v1) — getPool(монета, встречный,
//! swapFeeUnits) по уровням dex.v3_fee_tiers (как V3; Elastic Legacy на Arbitrum нет у индексатора).
//! Решение 482: Liquidity Book V2.1 — getAllLBPairs(монета, встречный) (factory_ask.rs Q::Lb).
//! Maverick V2 (решение 122 F) — lookup(tokenA, tokenB, 0, dex.pair_lookup_max) (не задана — по фабрике не ищется).
//! PancakeSwap Stable / Lista: пара с обёрткой BNB спрашивается и с меткой 0xEeee… (пул с нативным BNB фабрика хранит
//! под меткой; решение «122 X (нативный BNB в стейблсвапах)», stable_native.rs) — найденный пул записан с обёрткой.
//! WooFi (решение 305, при dex.multi_pair = да): state(монета) оракула — только со встречным = quoteToken() контракта
//! dex.pmm_pool (woo_quote.rs); не задана / нет — со всеми встречными, как раньше.
//! Настройки: dex.factory, dex.v3_fee_tiers, dex.cl_tick_spacings, dex.pair_lookup_max, dex.pmm_pool, dex.multi_pair [DEX]; через classify — dex.kind, dex.pool_enabled и адреса dex.*.
//!
//! Сигнатуры — официальные интерфейсы фабрик (abi_v2v3.rs):
//! getPair(address,address) returns (address) — https://github.com/Uniswap/v2-core/blob/6a9e7c97860676e0992f22a49665760444c1cdf5/contracts/interfaces/IUniswapV2Factory.sol#L9
//! (PancakeSwap — https://github.com/pancakeswap/pancake-swap-core/blob/3b214306770e86bc3a64e67c2b5bdb566b4e94a7/contracts/interfaces/IPancakeFactory.sol#L4-L11 );
//! getPool(address,address,uint24) returns (address) — https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/interfaces/IUniswapV3Factory.sol#L48
//! (PancakeSwap — IPancakeV3Factory.sol#L34-L70, pancake-v3-contracts 98684794). Порядок токенов любой: фабрика
//! хранит пару в обе стороны (getPair[token0][token1] и getPair[token1][token0] — UniswapV2Factory.sol createPair;
//! getPool[token0][token1][fee] и обратно — UniswapV3Factory.sol createPool). Нет пула — нулевой адрес.

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::{I24, U24};
use alloy_primitives::{Address, Bytes};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::abi_v2v3::{IV2Factory, IV3Factory};
use super::factory_ask::{Q, encode_q};
use super::families::{self, Family, Proto};
use super::indexer_gap;
use super::multi_pair;
use super::stable_native;
use super::woo_quote::{self, WooQuotes};

/// Предел uint24 (комиссия V3 в getPool) — из ABI: uint24 = 24 бита.
const U24_MAX: u32 = (1 << 24) - 1;

/// Один вопрос к фабрике: DEX, монета, встречный токен, вид вопроса (V3 — с уровнем комиссии).
#[derive(Debug, Clone)]
pub struct Ask {
    pub family: &'static Family,
    pub coin: Address,
    pub quote: Address,
    pub q: Q,
}

/// Все вопросы прохода: вызовы (фабрика, данные) по порядку вопросов и заметки по DEX.
#[derive(Debug, Default)]
pub struct Plan {
    pub asks: Vec<Ask>,
    pub calls: Vec<(Address, Bytes)>,
    pub notes: Vec<String>,
}

/// Собирает вопросы по всем подключённым DEX вида V2 / V3 / Solidly / Algebra / DODO. Принимает: монеты подписки, встречные токены (USDT и
/// промежуточные), котируемые монеты контрактов WooFi с цепи (woo_quote.rs read; решение 305). Возвращает: план.
/// Ошибки: Internal (dex.factory не читается у подключённой DEX — не бывает после classify, но не молчим).
pub fn plan(
    ctx: &Ctx,
    coins: &[Address],
    quotes: &[Address],
    woo: &WooQuotes,
) -> GatewayResult<Plan> {
    let mut out = Plan::default();
    for f in families::all(ctx) {
        if matches!(f.variant,super::family_variant::Variant::Carbon | super::family_variant::Variant::Temple){continue;} // pairs() у Controller перечисляется в carbon_directory
        // Решение 475 (T462-01): KyberSwap Elastic — getPool(a, b, swapFeeUnits) фабрики Elastic по уровням
        // dex.v3_fee_tiers: та же сигнатура getPool(address,address,uint24), что у V3, реестр — в обе стороны
        // (ks-elastic-sc 4ab08c0a contracts/Factory.sol L44, createPool L106-L109; фабрика v1 — Sourcify 42161
        // 0x5F1d…D50a, тот же код). Перечисления пулов у фабрики нет — только реестр по парам.
        let v3 = matches!(
            f.proto,
            Proto::PancakeV3 | Proto::UniswapV3 | Proto::KyberElastic
        );
        // Решение 450 (T448-05): iZiSwap — pool(a, b, fee) по уровням dex.v3_fee_tiers (как V3).
        let izi = f.proto == Proto::IziSwap;
        let mut qs: Vec<Q> = match f.proto {
            // Nomiswap Stable (решение 284): реестр фабрики getPair(a, b) — как у V2.
            // SmarDex (решение 288): getPair(a, b) — как V2 (SmardexFactory.sol L20).
            // Camelot V2 (решение 443): getPair(a, b) — как V2 (CamelotFactory.sol L19).
            Proto::V2 | Proto::NomiStable | Proto::Smardex | Proto::CamelotV2 => vec![Q::V2],
            // Решение 470: Aero Basic — getPool(a, b) фабрики своей кривой.
            Proto::AeroBasic => vec![Q::AeroBasic],
            // KyberSwap Classic DMM (решение 287): getPools(a, b) — все пулы пары.
            Proto::KyberDmm => vec![Q::Dmm],
            // Решение 482: Liquidity Book V2.1 — getAllLBPairs(a, b) только у DEX без справочника индексатора
            // (indexer_gap.rs; Swapline V1 в сети arbitrum): у DEX из списка индексатора (LFJ BSC и др.) — как раньше,
            // без поиска. V2.2 / v2.0 — отдельные виды, их фабрики здесь не сверялись.
            Proto::LbV20 | Proto::LbV22 if f.variant == super::family_variant::Variant::SectorLb => vec![Q::Lb],
            Proto::LbV21 => match indexer_gap::outside(ctx, f.id) {
                Some(true) => vec![Q::Lb],
                Some(false) => continue,
                None => {
                    out.notes.push(indexer_gap::why_not(f.id, None));
                    continue;
                }
            },
            // Curve Twocrypto-NG / Tricrypto-NG (решение 442) и crypto v1 фабрики (решение 453):
            // find_pool_for_coins(a, b, i), i < dex.pair_lookup_max.
            Proto::CurveTwocrypto | Proto::CurveTricrypto | Proto::CurveCryptoV1 => Vec::new(),
            Proto::PancakeV3 | Proto::UniswapV3 | Proto::IziSwap | Proto::KyberElastic => {
                Vec::new()
            }
            p if p.is_solidly() => {
                let v1 = p == Proto::SolidlyV1;
                [true, false]
                    .map(|stable| Q::Solidly { v1, stable })
                    .to_vec()
            }
            Proto::Algebra | Proto::AlgebraIntegral | Proto::AlgebraDirFee => vec![Q::Algebra],
            Proto::Slipstream => Vec::new(),
            Proto::Maverick => Vec::new(),
            p if p.is_dodo() => vec![Q::Dodo],
            // Решение 122 F, 3: PancakeSwap StableSwap — двух- и трёхмонетный реестр; Lista — getPairInfos;
            // WooFi — вопрос оракулу (адрес общего контракта — dex.pmm_pool, проверяется classify ниже).
            Proto::PancakeStable => vec![
                Q::PancakeStable { three: false },
                Q::PancakeStable { three: true },
            ],
            Proto::ListaStable => vec![Q::ListaStable],
            Proto::BaiBai | Proto::Elfomo | Proto::Lunar => Vec::new(),
            Proto::WooPP => match ctx.dex_addr("dex.pmm_pool", f.id) {
                Ok(pool) => vec![Q::Woo(pool)],
                Err(_) => Vec::new(),
            },
            _ => continue,
        };
        if let Err(why) = families::classify(ctx, f.id) {
            out.notes.push(format!("{}: не ищется — {why}", f.id));
            continue;
        }
        if f.variant == super::family_variant::Variant::BunnySwap {
            let (token,wrapped)=super::bunnyswap::tokens(ctx,f)?;
            let router=ctx.dex_addr("dex.quoter",f.id)?;
            for (coin,quote) in [(token,wrapped),(wrapped,token)] {
                if coins.contains(&coin) && quotes.contains(&quote) {
                    out.asks.push(Ask{family:f,coin,quote,q:Q::V2});
                    out.calls.push((router,super::bunnyswap::IRabbit::pairCall{}.abi_encode().into()));
                }
            }
            continue;
        }
        // WooFi: «реестр» — оракул (state(монета)); остальные — фабрика DEX.
        let registry = if matches!(f.proto, Proto::BaiBai | Proto::Elfomo | Proto::Lunar) {
            "dex.pmm_pool"
        } else if f.proto == Proto::WooPP {
            "dex.oracle"
        } else {
            "dex.factory"
        };
        let factory = ctx.dex_addr(registry, f.id)?;
        let ring = (f.variant == super::family_variant::Variant::RingFew)
            .then(|| super::ring::active(ctx, f.id))
            .transpose()?;
        if f.proto == Proto::Slipstream {
            match spacings(ctx, f.id) {
                Ok(list) => qs = list.into_iter().map(Q::Slipstream).collect(),
                Err(why) => {
                    out.notes.push(format!("{}: {why}", f.id));
                    continue;
                }
            }
        }
        if matches!(
            f.proto,
            Proto::CurveTwocrypto | Proto::CurveTricrypto | Proto::CurveCryptoV1
        ) {
            match lookup_max(ctx, f.id) {
                Ok(n) => qs = (0..n).map(Q::CurveCrypto).collect(),
                Err(why) => {
                    out.notes.push(format!("{}: {why}", f.id));
                    continue;
                }
            }
        }
        if f.proto == Proto::Maverick {
            match lookup_max(ctx, f.id) {
                Ok(n) => qs = vec![Q::Maverick(n)],
                Err(why) => {
                    out.notes.push(format!("{}: {why}", f.id));
                    continue;
                }
            }
        }
        if v3 || izi {
            match tiers(ctx, f.id) {
                Ok(list) if izi => qs = list.into_iter().map(Q::Izi).collect(),
                Ok(list) => qs = list.into_iter().map(Q::V3).collect(),
                Err(why) => {
                    out.notes.push(format!("{}: {why}", f.id));
                    continue;
                }
            }
        }
        // Решение 305 (dex.multi_pair = да у WooFi, multi_pair::woo_pairs): пара WooFi — (монета, quoteToken() КОНТРАКТА,
        // IWooPPV2.sol L67), state(монета) спрашивается только с этим встречным. Не прочитан (узел, откат) или не среди
        // встречных токенов подписки — WooFi этого прохода не ищется. Режим не включён — со всеми встречными, как раньше.
        let woo_q = if f.proto == Proto::WooPP && multi_pair::woo_pairs(ctx, f) {
            match woo_quote::quote_of(woo, f.id, ctx.dex_addr("dex.pmm_pool", f.id).ok()) {
                Some(q) if quotes.contains(&q) => Some(q),
                Some(q) => {
                    out.notes.push(format!(
                        "{}: quoteToken() контракта {q:#x} — не среди встречных токенов подписки — пары не ищутся",
                        f.id
                    ));
                    continue;
                }
                None => {
                    out.notes.push(format!(
                        "{}: quoteToken() контракта dex.pmm_pool не прочитан — пары не ищутся",
                        f.id
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        let pmm = if matches!(f.proto, Proto::Elfomo | Proto::BaiBai | Proto::Lunar) {
            match super::pmm_profile::Profile::read(ctx, f.id, f.proto) {
                Ok(p) if p.pool == factory => Some(p),
                _ => {
                    out.notes.push(format!(
                        "{}: набор рабочих параметров PMM неполон или противоречив",
                        f.id
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        let before = out.calls.len();
        let stable_u = matches!(f.proto, Proto::PancakeStable | Proto::ListaStable);
        let wrapped = stable_native::wrapped(ctx);
        for &coin in coins {
            for &quote in quotes
                .iter()
                .filter(|q| **q != coin && woo_q.is_none_or(|w| **q == w))
            {
                // Вопрос с той парой, что спрашиваем, и (стейблсвап Pancake / Lista) — с меткой BNB вместо обёртки.
                let alt = stable_u
                    .then(|| stable_native::registry_pair(coin, quote, wrapped))
                    .flatten();
                for (a, b) in std::iter::once((coin, quote)).chain(alt) {
                    if f.variant == super::family_variant::Variant::MintClub {
                        qs.clear(); qs.extend([Q::MintClub{token:a,reserve:b},Q::MintClub{token:b,reserve:a}]);
                    }
                    if let Some(p) = &pmm {
                        if !p.pair(a, b) {
                            continue;
                        }
                        qs.clear();
                        qs.push(match f.proto {
                            Proto::BaiBai => Q::BaiBai(factory, p.counter),
                            Proto::Lunar => Q::Lunar(factory, p.counter),
                            _ => Q::Elfomo(factory, a, b),
                        });
                    }
                    let (a, b) = if let Some(layout) = &ring {
                        let Some([x, y]) = layout.path(a, b) else {
                            continue;
                        };
                        (x, y)
                    } else {
                        (a, b)
                    };
                    for &q in &qs {
                        out.calls.push((factory, encode_q(q, a, b)));
                        out.asks.push(Ask {
                            family: f,
                            coin,
                            quote,
                            q,
                        });
                    }
                }
            }
        }
        let kind = qs.first().map_or("", |q| q.name());
        let n = out.calls.len() - before;
        out.notes.push(format!("{}: {kind} — {n} вызовов", f.id));
    }
    Ok(out)
}

/// Уровни комиссии V3 этой DEX из dex.v3_fee_tiers (список целых uint24, как fee в getPool).
/// Возвращает: список или текст причины, почему V3 этой DEX по фабрике не ищется. Ошибок нет.
pub fn tiers(ctx: &Ctx, id: &str) -> Result<Vec<u32>, String> {
    let list = match ctx.dex_list("dex.v3_fee_tiers", id) {
        Ok(l) => l,
        Err(e) if e.code == ErrorCode::NotConfigured => {
            return Err("не задана dex.v3_fee_tiers — V3 этой DEX по фабрике не ищется".into());
        }
        Err(e) => return Err(format!("dex.v3_fee_tiers: {}", e.message)),
    };
    if list.is_empty() {
        return Err("dex.v3_fee_tiers пуст — V3 этой DEX по фабрике не ищется".into());
    }
    list.iter()
        .map(|s| match s.trim().parse::<u32>() {
            Ok(v) if v <= U24_MAX => Ok(v),
            _ => Err(format!(
                "dex.v3_fee_tiers: «{s}» — не целое uint24 (fee в getPool) — V3 этой DEX по фабрике не ищется"
            )),
        })
        .collect()
}

/// Уровни tickSpacing Slipstream этой DEX из dex.cl_tick_spacings (список целых int24, как tickSpacing в getPool).
/// Возвращает: список или текст причины, почему Slipstream этой DEX по фабрике не ищется. Ошибок нет.
pub fn spacings(ctx: &Ctx, id: &str) -> Result<Vec<I24>, String> {
    let list = match ctx.dex_list("dex.cl_tick_spacings", id) {
        Ok(l) => l,
        Err(e) if e.code == ErrorCode::NotConfigured => {
            return Err(
                "не задана dex.cl_tick_spacings — Slipstream этой DEX по фабрике не ищется".into(),
            );
        }
        Err(e) => return Err(format!("dex.cl_tick_spacings: {}", e.message)),
    };
    if list.is_empty() {
        return Err("dex.cl_tick_spacings пуст — Slipstream этой DEX по фабрике не ищется".into());
    }
    // int24: −2^23 ..= 2^23 − 1 (ABI int24) — I24::try_from отвергает значение вне диапазона.
    list.iter()
        .map(|s| match s.trim().parse::<i32>().ok().and_then(|v| I24::try_from(v).ok()) {
            Some(v) => Ok(v),
            None => Err(format!(
                "dex.cl_tick_spacings: «{s}» — не целое int24 — Slipstream этой DEX по фабрике не ищется"
            )),
        })
        .collect()
}

/// Предел lookup(tokenA, tokenB, 0, N) Maverick V2 из dex.pair_lookup_max этой DEX. Возвращает: N или текст причины,
/// почему Maverick V2 этой DEX по фабрике не ищется. Ошибок нет.
pub fn lookup_max(ctx: &Ctx, id: &str) -> Result<u32, String> {
    match ctx.dex_int("dex.pair_lookup_max", id) {
        Ok(n) => u32::try_from(n).map_err(|_| "dex.pair_lookup_max слишком велик".to_string()),
        Err(e) if e.code == ErrorCode::NotConfigured => {
            Err("не задана dex.pair_lookup_max — пулы этой DEX по фабрике не ищутся".into())
        }
        Err(e) => Err(format!("dex.pair_lookup_max: {}", e.message)),
    }
}

/// Данные вызова: V2 getPair(coin, quote); V3 getPool(coin, quote, fee). Ошибок нет (fee уже проверен на uint24).
pub fn encode(coin: Address, quote: Address, fee: Option<u32>) -> Bytes {
    match fee {
        None => IV2Factory::getPairCall {
            tokenA: coin,
            tokenB: quote,
        }
        .abi_encode()
        .into(),
        Some(fee) => IV3Factory::getPoolCall {
            tokenA: coin,
            tokenB: quote,
            fee: U24::from(fee),
        }
        .abi_encode()
        .into(),
    }
}

/// Разбор ответа: адрес пула (нулевой — пула нет). Принимает: уровень комиссии вопроса (None — V2 getPair), байты
/// ответа. Возвращает: адрес или текст «ответ не по ABI». Ошибок нет.
pub fn decode(fee: Option<u32>, out: &[u8]) -> Result<Address, String> {
    match fee {
        None => IV2Factory::getPairCall::abi_decode_returns(out)
            .map_err(|e| format!("ответ getPair не по ABI: {e}")),
        Some(_) => IV3Factory::getPoolCall::abi_decode_returns(out)
            .map_err(|e| format!("ответ getPool не по ABI: {e}")),
    }
}
