//! Файл: venues/bsc_dex/pools/factory_calls.rs
//! Что делает: вызовы реестра фабрик для поиска пулов на своём узле (решение 121, C3): по каждой подключённой DEX
//! вида V2 / форк V2 — getPair(монета, встречный токен); V3 (PancakeSwap / Uniswap ABI) — getPool(монета, встречный,
//! fee) по каждому уровню комиссии из dex.v3_fee_tiers этой DEX (не задана — V3 этой DEX по фабрике не ищется).
//! DEX не подключена (families::classify) — не ищется, причина в заметках прохода. Только сборка вызовов и разбор
//! ответа — без сети.
//! Кто вызывает: pools/factory_scan.rs.
//! От чего зависит: families.rs (all, classify, Proto), abi_v2v3.rs (IV2Factory, IV3Factory), ctx.rs.
//! Настройки: dex.factory, dex.v3_fee_tiers [DEX]; через classify — dex.kind, dex.pool_enabled и адреса dex.*.
//!
//! Сигнатуры — официальные интерфейсы фабрик (abi_v2v3.rs):
//! getPair(address,address) returns (address) — https://github.com/Uniswap/v2-core/blob/6a9e7c97860676e0992f22a49665760444c1cdf5/contracts/interfaces/IUniswapV2Factory.sol#L9
//! (PancakeSwap — https://github.com/pancakeswap/pancake-swap-core/blob/3b214306770e86bc3a64e67c2b5bdb566b4e94a7/contracts/interfaces/IPancakeFactory.sol#L4-L11 );
//! getPool(address,address,uint24) returns (address) — https://github.com/Uniswap/v3-core/blob/5ea68a77cb541c6b3812df783be71ba8c081e25f/contracts/interfaces/IUniswapV3Factory.sol#L48
//! (PancakeSwap — IPancakeV3Factory.sol#L34-L70, pancake-v3-contracts 98684794). Порядок токенов любой: фабрика
//! хранит пару в обе стороны (getPair[token0][token1] и getPair[token1][token0] — UniswapV2Factory.sol createPair;
//! getPool[token0][token1][fee] и обратно — UniswapV3Factory.sol createPool). Нет пула — нулевой адрес.

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::abi_v2v3::{IV2Factory, IV3Factory};
use super::families::{self, Family, Proto};

/// Предел uint24 (комиссия V3 в getPool) — из ABI: uint24 = 24 бита.
const U24_MAX: u32 = (1 << 24) - 1;

/// Один вопрос к фабрике: DEX, монета, встречный токен, уровень комиссии (только V3).
#[derive(Debug, Clone)]
pub struct Ask {
    pub family: &'static Family,
    pub coin: Address,
    pub quote: Address,
    pub fee: Option<u32>,
}

/// Все вопросы прохода: вызовы (фабрика, данные) по порядку вопросов и заметки по DEX.
#[derive(Debug, Default)]
pub struct Plan {
    pub asks: Vec<Ask>,
    pub calls: Vec<(Address, Bytes)>,
    pub notes: Vec<String>,
}

/// Собирает вопросы по всем подключённым DEX вида V2 / V3. Принимает: монеты подписки, встречные токены (USDT и
/// промежуточные). Возвращает: план. Ошибки: Internal (dex.factory не читается у подключённой DEX — не бывает
/// после classify, но не молчим).
pub fn plan(ctx: &Ctx, coins: &[Address], quotes: &[Address]) -> GatewayResult<Plan> {
    let mut out = Plan::default();
    for f in families::all(ctx) {
        let v3 = match f.proto {
            Proto::V2 => false,
            Proto::PancakeV3 | Proto::UniswapV3 => true,
            _ => continue,
        };
        if let Err(why) = families::classify(ctx, f.id) {
            out.notes.push(format!("{}: не ищется — {why}", f.id));
            continue;
        }
        let factory = ctx.dex_addr("dex.factory", f.id)?;
        let fees: Vec<Option<u32>> = if v3 {
            match tiers(ctx, f.id) {
                Ok(list) => list.into_iter().map(Some).collect(),
                Err(why) => {
                    out.notes.push(format!("{}: {why}", f.id));
                    continue;
                }
            }
        } else {
            vec![None]
        };
        let before = out.calls.len();
        for &coin in coins {
            for &quote in quotes.iter().filter(|q| **q != coin) {
                for &fee in &fees {
                    out.calls.push((factory, encode(coin, quote, fee)));
                    out.asks.push(Ask {
                        family: f,
                        coin,
                        quote,
                        fee,
                    });
                }
            }
        }
        let kind = if v3 { "getPool" } else { "getPair" };
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
