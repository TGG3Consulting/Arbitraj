//! Файл: venues/bsc_dex/pools/factory_ask.rs
//! Что делает: вид вопроса к реестру фабрики при поиске пулов на своём узле (решение 121 C3, дополнено решением 122 F):
//! V2 getPair, V3 getPool по уровню комиссии, Solidly getPool / Solidly V1 (THENA V1) getPair по флагу stable, Algebra
//! poolByPair, DODO V2 getDODOPoolBidirection (DVM/DPP/DSP — индексатор их пулы на BSC не отдаёт, карта покрытия).
//! Сборка данных вызова и разбор ответа в список адресов пулов — без сети.
//! Кто вызывает: pools/factory_calls.rs (plan), pools/factory_scan.rs (разбор ответов).
//! От чего зависит: factory_calls.rs (encode/decode V2/V3), verify_solidly.rs (registry_call), abi_algebra.rs,
//! abi_dodo.rs, abi_solidly.rs.
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
//!   с base = token1; оба — пулы этой пары.

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::I24;
use alloy_primitives::{Address, Bytes};

use super::abi_algebra::IAlgebraFactory;
use super::abi_dodo::IDodoFactory;
use super::abi_slipstream::ICLFactory;
use super::abi_solidly::ISolidlyFactory;
use super::abi_stable_u::{IListaStableFactory, IPancakeStableFactory};
use super::abi_woo::IWooracle;
use super::factory_calls::{decode, encode};
use super::verify_solidly::registry_call;

/// Вид вопроса к реестру фабрики.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Q {
    /// V2 и форки V2: getPair(a, b).
    V2,
    /// V3 (ABI PancakeSwap / Uniswap, форки V3): getPool(a, b, fee).
    V3(u32),
    /// Solidly: v1 = false — getPool(a, b, stable) (Velodrome), v1 = true — getPair(a, b, stable) (THENA V1).
    Solidly { v1: bool, stable: bool },
    /// Algebra (THENA Fusion): poolByPair(a, b).
    Algebra,
    /// Slipstream: getPool(a, b, int24 tickSpacing) по уровню из dex.cl_tick_spacings (уже int24 — factory_calls.rs
    /// spacings отвергает значение вне int24 с его текстом; ревью 122, пункт 6).
    Slipstream(I24),
    /// DODO V2 (DVM/DPP/DSP): getDODOPoolBidirection(a, b).
    Dodo,
    /// PancakeSwap StableSwap V2 (решение 122 F, 3): getPairInfo(a, b) — двухмонетный пул; three = true —
    /// getThreePoolPairInfo(a, b) — трёхмонетный (PancakeStableSwapFactory.sol L194, L203).
    PancakeStable { three: bool },
    /// Lista StableSwap: getPairInfos(a, b) — все пулы пары (StableSwapFactory.sol L206).
    ListaStable,
    /// WooFi (решение 122 F, 3): оракул state(монета) — цена > 0 значит, монета — базовая общего контракта `pool`
    /// (IWooracleV2_2.sol). Вопрос задаётся оракулу (dex.oracle), найденный пул — общий контракт dex.pmm_pool.
    Woo(Address),
}

impl Q {
    /// Имя вызова для заметок прохода. Ошибок нет.
    pub fn name(self) -> &'static str {
        match self {
            Q::V2 => "getPair",
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
        }
    }
}

/// Данные вызова вопроса. Принимает: вид вопроса, монету, встречный токен. Ошибок нет.
pub fn encode_q(q: Q, coin: Address, quote: Address) -> Bytes {
    match q {
        Q::V2 => encode(coin, quote, None),
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
        Q::Woo(_) => IWooracle::stateCall { base: coin }.abi_encode().into(),
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
        Q::V2 => vec![decode(None, out)?],
        Q::V3(fee) => vec![decode(Some(fee), out)?],
        // Ответ getPool и getPair(stable) — один address: разбор одинаков.
        Q::Solidly { .. } => vec![
            ISolidlyFactory::getPoolCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ {} не по ABI: {e}", q.name()))?,
        ],
        Q::Algebra => vec![
            IAlgebraFactory::poolByPairCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ poolByPair не по ABI: {e}"))?,
        ],
        Q::Slipstream(_) => vec![
            ICLFactory::getPoolCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ getPool(tickSpacing) не по ABI: {e}"))?,
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
        Q::Woo(pool) => {
            let st = IWooracle::stateCall::abi_decode_returns(out)
                .map_err(|e| format!("ответ state(base) не по ABI: {e}"))?;
            if st.price > 0 { vec![pool] } else { Vec::new() }
        }
    };
    Ok(list.into_iter().filter(|a| *a != Address::ZERO).collect())
}
