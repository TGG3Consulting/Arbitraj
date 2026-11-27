//! Файл: venues/bsc_dex/pools/factory_ask.rs
//! Что делает: вид вопроса к реестру фабрики при поиске пулов на своём узле (решение 121 C3, дополнено решением 122 F):
//! V2 getPair, V3 getPool по уровню комиссии, Solidly getPool / Solidly V1 (THENA V1) getPair по флагу stable, Algebra
//! poolByPair, DODO V2 getDODOPoolBidirection (DVM/DPP/DSP — индексатор их пулы на BSC не отдаёт, карта покрытия),
//! Maverick V2 lookup(tokenA, tokenB, 0, N) (решение 122 F).
//! Сборка данных вызова и разбор ответа в список адресов пулов — без сети.
//! Кто вызывает: pools/factory_calls.rs (plan), pools/factory_scan.rs (разбор ответов).
//! От чего зависит: factory_calls.rs (encode/decode V2/V3), verify_solidly.rs (registry_call), abi_algebra.rs,
//! abi_dodo.rs, abi_solidly.rs, abi_maverick.rs.
//! Настройки: нет (адрес фабрики и уровни комиссии — factory_calls.rs).
//!
//! Сигнатуры — официальные интерфейсы фабрик:
//! - Solidly (Velodrome) getPool(address,address,bool) — velodrome-finance/contracts b3065d8, PoolFactory.sol L68-L71;
//!   Solidly V1 getPair(address,address,bool) — ThenafiBNB/THENA-Contracts f5bc742, PairFactoryUpgradeable.sol L24
//!   (оба реестра заполнены в обе стороны токенов — порядок любой; нет пула — нулевой адрес);
//! - Slipstream getPool(address,address,int24 tickSpacing) — velodrome-finance/slipstream, ICLFactory (abi_slipstream.rs);
//! - Algebra poolByPair(address,address) — @cryptoalgebra/v1.9-core IAlgebraFactory.sol (abi_algebra.rs);
//! - DODO getDODOPoolBidirection(token0, token1) → (base0[], base1[]) — DODOEX/contractV2 2f1bcda,
//!   Factory/DVMFactory.sol, DPPFactory.sol, DSPFactory.sol (abi_dodo.rs): base0 — пулы с base = token0, base1 —
//!   с base = token1; оба — пулы этой пары;
//! - Maverick V2 lookup(IERC20 _tokenA, IERC20 _tokenB, uint256 startIndex, uint256 endIndex) → пулы[] —
//!   maverickprotocol/v2-common d91e871, IMaverickV2Factory.sol L352-L357 (постраничный список пулов пары —
//!   maverick-v2-examples 68e4d2f README.md L8-L25); endIndex — dex.pair_lookup_max;
//! - KyberSwap Classic DMM getPools(token0, token1) → пулы[] — KyberNetwork/dmm-smart-contracts@e557b57, DMMFactory.sol
//!   L94-L105 (решение 287);
//! - iZiSwap pool(address,address,uint24) → пул — izumiFinance/iZiSwap-core@3fe21eba iZiSwapFactory.sol L24
//!   (mapping tokenX ⇒ tokenY ⇒ fee), newPool L123-L124 — обе стороны; развёрнутый код Arbitrum 0xCFD8A067… — Sourcify
//!   42161 (solc 0.8.4, тот же текст; решение 450);
//! - Curve Twocrypto-NG / Tricrypto-NG find_pool_for_coins(_from, _to, i) → пул (решение 442) — curvefi/twocrypto-ng
//!   89c8dfb CurveTwocryptoFactory.vy L376-L386, curvefi/tricrypto-ng ecaa816 CurveL2TricryptoFactory.vy L369-L379;
//!   crypto v1 фабрики (решение 453) — curvefi/curve-factory-crypto 7ed1011 Factory.vy L335-L345: `markets[from ^
//!   to][i]` — порядок токенов любой (ключ — XOR адресов). За последним пулом пары: у Twocrypto-NG — откат (markets —
//!   DynArray, CurveTwocryptoFactory.vy L91), у Tricrypto-NG L2 и фабрики v1 — нулевой адрес (address[4294967296],
//!   CurveL2TricryptoFactory.vy L89, Factory.vy L117). factory_scan.rs не считает ошибкой откат при i > 0; нулевой
//!   адрес отбрасывается как «пула нет».
//! - Решение 482: Liquidity Book V2.1 getAllLBPairs(tokenX, tokenY) → LBPairInformation[] (все шаги бина пары) —
//!   lfj-gg/joe-v2@v2.1.0 src/LBFactory.sol L265 (токены сортируются — порядок любой), = Sourcify exact_match LBFactory
//!   Swapline (Arbitrum) и фабрики BSC (abi_lb.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::{I24, U24};
use alloy_primitives::{Address, Bytes};

use super::abi_aero_basic::IAeroBasicFactory;
use super::abi_algebra::IAlgebraFactory;
use super::abi_crypto::ICryptoFactory;
use super::abi_dmm::IDmmFactory;
use super::abi_dodo::IDodoFactory;
use super::abi_izi::IIziFactory;
use super::abi_lb::ILBFactory;
use super::abi_maverick::IMaverickV2Factory;
use super::abi_slipstream::ICLFactory;
use super::abi_solidly::ISolidlyFactory;
use super::abi_stable_u::{IListaStableFactory, IPancakeStableFactory};
use super::abi_woo::IWooracle;
use super::factory_calls::{decode, encode};
use super::verify_solidly::registry_call;

/// Вид вопроса к реестру фабрики.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Q {
    Rubicon(Address, bool),
    /// V2 и форки V2: getPair(a, b).
    V2,
    UmbraeBins,
    Antfarm(u16),
    MintClub {token:Address,reserve:Address},
    /// V3 (ABI PancakeSwap / Uniswap, форки V3): getPool(a, b, fee).
    V3(u32),
    /// Solidly: v1 = false — getPool(a, b, stable) (Velodrome), v1 = true — getPair(a, b, stable) (THENA V1).
    Solidly {
        v1: bool,
        stable: bool,
    },
    /// Algebra (THENA Fusion): poolByPair(a, b).
    Algebra,
    /// Slipstream: getPool(a, b, int24 tickSpacing) по уровню из dex.cl_tick_spacings (уже int24 — factory_calls.rs
    /// spacings отвергает значение вне int24 с его текстом; ревью 122, пункт 6).
    Slipstream(I24),
    /// DODO V2 (DVM/DPP/DSP): getDODOPoolBidirection(a, b).
    Dodo,
    /// PancakeSwap StableSwap V2 (решение 122 F, 3): getPairInfo(a, b) — двухмонетный пул; three = true —
    /// getThreePoolPairInfo(a, b) — трёхмонетный (PancakeStableSwapFactory.sol L194, L203).
    PancakeStable {
        three: bool,
    },
    /// Lista StableSwap: getPairInfos(a, b) — все пулы пары (StableSwapFactory.sol L206).
    ListaStable,
    /// WooFi (решение 122 F, 3): оракул state(монета) — цена > 0 значит, монета — базовая общего контракта `pool`
    /// (IWooracleV2_2.sol). Вопрос задаётся оракулу (dex.oracle), найденный пул — общий контракт dex.pmm_pool.
    Woo(Address),
    Elfomo(Address, Address, Address),
    Lunar(Address, Address),
    BaiBai(Address, Address),
    /// Maverick V2: lookup(tokenA, tokenB, 0, предел) — все пулы пары (предел — dex.pair_lookup_max).
    Maverick(u32),
    /// KyberSwap Classic DMM (решение 287): getPools(a, b) — все пулы пары (с разным усилением).
    Dmm,
    /// Решение 450 (T448-05): iZiSwap — pool(a, b, uint24 fee) по уровню из dex.v3_fee_tiers.
    Izi(u32),
    /// Curve Twocrypto-NG / Tricrypto-NG (решение 442), crypto v1 фабрики (решение 453): find_pool_for_coins(a, b, i) —
    /// i-й пул пары.
    CurveCrypto(u32),
    /// Решение 470: Aero Basic — getPool(a, b) фабрики своей кривой (metadex-public 1.1.0 PoolFactory.sol L159-L161).
    AeroBasic,
    /// Решение 482: Liquidity Book V2.1 — getAllLBPairs(a, b), все пулы пары (по шагам бина).
    Lb,
}

impl Q {
    /// Имя вызова для заметок прохода. Ошибок нет.
    pub fn name(self) -> &'static str {
        match self {
            Q::MintClub{..} => "tokenBond",
            Q::Rubicon(..) => "getBestOffer",
            Q::V2 => "getPair",
            Q::UmbraeBins => "getAllPairs(tokenA,tokenB)",
            Q::Antfarm(_) => "getPair(tokenA,tokenB,fee)",
            Q::V3(_) => "getPool",
            Q::Solidly { v1: false, .. } => "getPool(stable)",
            Q::Solidly { v1: true, .. } => "getPair(stable)",
            Q::Algebra => "poolByPair",
            Q::Slipstream(_) => "getPool(tickSpacing)",
            Q::Dodo => "getDODOPoolBidirection",
            Q::PancakeStable { three: false } => "getPairInfo",
            Q::PancakeStable { three: true } => "getThreePoolPairInfo",
            Q::ListaStable => "getPairInfos",
            Q::Woo(_) => "state(base)",
            Q::BaiBai(..) => "quoteToken",
            Q::Lunar(..) => "Y",
            Q::Elfomo(..) => "getSupportedPairs",
            Q::Maverick(_) => "lookup(tokenA, tokenB)",
            Q::Dmm => "getPools",
            Q::Izi(_) => "pool(tokenX, tokenY, fee)",
            Q::CurveCrypto(_) => "find_pool_for_coins",
            Q::AeroBasic => "getPool(a, b)",
            Q::Lb => "getAllLBPairs",
        }
    }
}

/// Данные вызова вопроса. Принимает: вид вопроса, монету, встречный токен. Ошибок нет.
pub fn encode_q(q: Q, coin: Address, quote: Address) -> Bytes {
    match q {
        Q::MintClub{token,..} => super::mint_club::IBond::tokenBondCall{token}.abi_encode().into(),
        Q::Rubicon(_,reverse) => {
            let (sellToken,buyToken)=if reverse{(quote,coin)}else{(coin,quote)};
            super::rubicon::IRubiconMarket::getBestOfferCall{sellToken,buyToken}.abi_encode().into()
        },
        Q::V2 => encode(coin, quote, None),
        Q::UmbraeBins => super::umbrae::IMarket::getAllPairsCall{a:coin,b:quote}.abi_encode().into(),
        Q::Antfarm(fee) => super::antfarm::IFarm::getPairCall{a:coin,b:quote,fee}.abi_encode().into(),
        Q::V3(fee) => encode(coin, quote, Some(fee)),
        Q::Solidly { v1, stable } => registry_call(v1, coin, quote, stable),
        Q::Algebra => IAlgebraFactory::poolByPairCall {
            tokenA: coin,
            tokenB: quote,
        }
        .abi_encode()
        .into(),
        Q::Dodo => IDodoFactory::getDODOPoolBidirectionCall {
            token0: coin,
            token1: quote,
        }
        .abi_encode()
        .into(),
        Q::PancakeStable { three: false } => IPancakeStableFactory::getPairInfoCall {
            _tokenA: coin,
            _tokenB: quote,
        }
        .abi_encode()
        .into(),
        Q::PancakeStable { three: true } => IPancakeStableFactory::getThreePoolPairInfoCall {
            _tokenA: coin,
            _tokenB: quote,
        }
        .abi_encode()
        .into(),
        Q::ListaStable => IListaStableFactory::getPairInfosCall {
            _tokenA: coin,
            _tokenB: quote,
        }
        .abi_encode()
        .into(),
        Q::BaiBai(..) => super::baibai::IBai::quoteTokenCall {}.abi_encode().into(),
        Q::Lunar(..) => super::lunar::ILunar::YCall {}.abi_encode().into(),
        Q::Elfomo(..) => super::elfomo::IElfomo::getSupportedPairsCall {}
            .abi_encode()
            .into(),
        Q::Woo(_) => IWooracle::stateCall { base: coin }.abi_encode().into(),
        // KyberSwap Classic DMM: реестр заполнен в обе стороны токенов (DMMFactory.sol createPool L52-L53) — порядок любой.
        Q::Dmm => IDmmFactory::getPoolsCall {
            token0: coin,
            token1: quote,
        }
        .abi_encode()
        .into(),
        // Maverick V2: tokenA < tokenB (IMaverickV2Pool.sol L231-L233; фабрика create откатывает обратный порядок —
        // FactoryInvalidTokenOrder, IMaverickV2Factory.sol L17), поэтому пара — по возрастанию адреса.
        Q::Maverick(end) => {
            let (a, b) = if coin < quote {
                (coin, quote)
            } else {
                (quote, coin)
            };
            IMaverickV2Factory::lookupCall {
                _tokenA: a,
                _tokenB: b,
                startIndex: alloy_primitives::U256::ZERO,
                endIndex: alloy_primitives::U256::from(end),
            }
            .abi_encode()
            .into()
        }
        // iZiSwap: реестр заполнен в обе стороны токенов (iZiSwapFactory.sol L123-L124) — порядок любой.
        Q::Izi(fee) => IIziFactory::poolCall {
            tokenX: coin,
            tokenY: quote,
            fee: U24::from(fee),
        }
        .abi_encode()
        .into(),
        Q::CurveCrypto(i) => ICryptoFactory::find_pool_for_coinsCall {
            from: coin,
            to: quote,
            i: alloy_primitives::U256::from(i),
        }
        .abi_encode()
        .into(),
        // Aero Basic: реестр заполнен в обе стороны токенов (PoolFactory.sol L334-L335) — порядок любой.
        Q::AeroBasic => IAeroBasicFactory::getPoolCall {
            tokenA: coin,
            tokenB: quote,
        }
        .abi_encode()
        .into(),
        Q::Lb => ILBFactory::getAllLBPairsCall {
            tokenX: coin,
            tokenY: quote,
        }
        .abi_encode()
        .into(),
        Q::Slipstream(ts) => ICLFactory::getPoolCall {
            tokenA: coin,
            tokenB: quote,
            tickSpacing: ts,
        }
        .abi_encode()
        .into(),
    }
}

/// Разбор ответа в адреса пулов (без нулевых). Принимает: вид вопроса, байты ответа. Возвращает: список адресов (пусто —
/// пула нет) или текст «ответ не по ABI». Ошибок нет.
pub fn decode_q(q: Q, out: &[u8]) -> Result<Vec<Address>, String> {
    let list = match q {
        Q::Rubicon(market,_) => {
            let id=super::rubicon::IRubiconMarket::getBestOfferCall::abi_decode_returns_validate(out).map_err(|e|e.to_string())?;
            if id.is_zero(){Vec::new()}else{vec![market]}
        }
        Q::MintClub{token,reserve} => super::mint_club::listed(token,reserve,out)?,
        Q::UmbraeBins => super::umbrae::IMarket::getAllPairsCall::abi_decode_returns(out).map_err(|err|err.to_string())?,
        Q::V2 | Q::Antfarm(_) => vec![decode(None, out)?],
        Q::V3(fee) => vec![decode(Some(fee), out)?],
        // Ответ getPool и getPair(stable) — один address: разбор одинаков.
        Q::Solidly { .. } => vec![
            ISolidlyFactory::getPoolCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ {} не по ABI: {e}", q.name()))?,
        ],
        Q::AeroBasic => vec![
            IAeroBasicFactory::getPoolCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ getPool(a, b) не по ABI: {e}"))?,
        ],
        Q::Algebra => vec![
            IAlgebraFactory::poolByPairCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ poolByPair не по ABI: {e}"))?,
        ],
        Q::Slipstream(_) => vec![
            ICLFactory::getPoolCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ getPool(tickSpacing) не по ABI: {e}"))?,
        ],
        Q::Maverick(_) => IMaverickV2Factory::lookupCall::abi_decode_returns(out)
            .map_err(|e| format!("ответ lookup(tokenA, tokenB) не по ABI: {e}"))?,
        Q::Izi(_) => vec![
            IIziFactory::poolCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ pool(tokenX, tokenY, fee) не по ABI: {e}"))?,
        ],
        // Ревью t251: пары с ignoredForRouting = да (фабрика сняла их с маршрутизации) не берутся.
        Q::Lb => ILBFactory::getAllLBPairsCall::abi_decode_returns(out)
            .map_err(|e| format!("ответ getAllLBPairs не по ABI: {e}"))?
            .into_iter()
            .filter(|i| !i.ignoredForRouting)
            .map(|i| i.LBPair)
            .collect(),
        Q::Dmm => IDmmFactory::getPoolsCall::abi_decode_returns(out)
            .map_err(|e| format!("ответ getPools не по ABI: {e}"))?,
        Q::CurveCrypto(_) => vec![
            ICryptoFactory::find_pool_for_coinsCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ find_pool_for_coins не по ABI: {e}"))?,
        ],
        Q::Dodo => {
            let r = IDodoFactory::getDODOPoolBidirectionCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ getDODOPoolBidirection не по ABI: {e}"))?;
            r.baseToken0Pools
                .into_iter()
                .chain(r.baseToken1Pools)
                .collect()
        }
        Q::PancakeStable { three: false } => vec![
            IPancakeStableFactory::getPairInfoCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ getPairInfo не по ABI: {e}"))?
                .swapContract,
        ],
        Q::PancakeStable { three: true } => vec![
            IPancakeStableFactory::getThreePoolPairInfoCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ getThreePoolPairInfo не по ABI: {e}"))?
                .swapContract,
        ],
        Q::ListaStable => IListaStableFactory::getPairInfosCall::abi_decode_returns(out)
            .map_err(|e| format!("ответ getPairInfos не по ABI: {e}"))?
            .into_iter()
            .map(|i| i.swapContract)
            .collect(),
        Q::BaiBai(pool, expected) => {
            let quote = super::baibai::IBai::quoteTokenCall::abi_decode_returns_validate(out)
                .map_err(|e| e.to_string())?;
            if quote == expected {
                vec![pool]
            } else {
                Vec::new()
            }
        }
        Q::Lunar(pool, expected) => {
            let quote = super::lunar::ILunar::YCall::abi_decode_returns_validate(out)
                .map_err(|e| e.to_string())?;
            if quote == expected {
                vec![pool]
            } else {
                Vec::new()
            }
        }
        Q::Elfomo(pool, coin, quote) => {
            if super::elfomo::listed(out, coin, quote)? {
                vec![pool]
            } else {
                Vec::new()
            }
        }
        Q::Woo(pool) => {
            let st = IWooracle::stateCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ state(base) не по ABI: {e}"))?;
            if st.price > 0 { vec![pool] } else { Vec::new() }
        }
    };
    Ok(list.into_iter().filter(|a| *a != Address::ZERO).collect())
}
