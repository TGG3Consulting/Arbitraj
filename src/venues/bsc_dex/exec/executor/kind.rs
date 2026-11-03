//! Файл: venues/bsc_dex/exec/executor/kind.rs
//! Что делает: ПУБЛИЧНЫЙ кодировщик шагов исполнителя ArbExecutor (решение 122, часть X) — один вход для всех семейств
//! DEX: вид шага с его параметрами (Params), пул, токены, доля входа (StepSpec) → шаг ABI контракта (abi::ArbExecutor::Step).
//! Часть F (новые семейства: Curve, WooFi, Maverick…) строит шаги только через `encode` — ABI и числа видов здесь.
//! Номера видов — константы контракта contracts/ArbExecutor.sol (V2 = 0 … ELASTIC = 8, STABLE_U256 = 9, SADDLE = 10,
//! STABLE_U256_BNB = 11; сверяет тест tests/bsc_dex_executor_abi.rs с исходником); раскладка data — комментарий Step в
//! контракте: V2 — abi.encode(uint32 feePips), CURVE — (int128 i, int128 j), STABLE_U256 — (uint256 i, uint256 j),
//! SADDLE — (uint8 i, uint8 j), STABLE_U256_BNB — (uint256 i, uint256 j, bool bnbIn), остальные — пусто.
//! Кто вызывает: exec/executor/{call, entry}.rs; часть F (новые семейства).
//! От чего зависит: exec/executor/abi.rs, alloy sol-types.
//! Настройки: нет.

use alloy::sol_types::sol_data::Uint;
use alloy::sol_types::{SolType, SolValue};
use alloy_primitives::{Address, Bytes, U256};

use super::super::super::pools::stable_native;
use super::abi::ArbExecutor;

/// Нативная монета в пулах StableSwap — BNB_ADDRESS PancakeSwap Stable (pancake-smart-contracts@cb079908
/// PancakeStableSwapTwoPool.sol L39) и Lista (moolah@eb7d73a StableSwapPool.sol L54); константа протокола, не настройка
/// (pools/stable_native.rs). Токеном шага быть не может (не ERC-20; контракт: require «native»): шаг через пул с BNB —
/// вид STABLE_U256_BNB с токеном шага WBNB.
pub const STABLE_NATIVE: Address = stable_native::NATIVE;

/// Вся доля: 10000 bps = весь текущий баланс исполнителя по токену входа (константа BPS контракта).
pub const ALL_BPS: u16 = 10_000;

/// Вид шага и его параметры — по официальному интерфейсу пула (ссылки — contracts/IPools.sol).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Params {
    /// Пара V2 и форки (PancakeSwap V2, Uniswap V2, Biswap…): комиссия пары в долях 1e-6 (2500 = 0,25 %).
    V2 { fee_pips: u32 },
    /// Пул V3: PancakeSwap V3, Uniswap V3, форки V3, Slipstream (Topaz) — swap + обратный вызов.
    V3,
    /// Пул Algebra V1.9 (THENA Fusion) — swap + algebraSwapCallback.
    Algebra,
    /// Пара Solidly (THENA V1, volatile и stable): выход — getAmountOut самой пары.
    Solidly,
    /// Плоский пул Curve / Ellipsis: exchange(int128 i, int128 j, dx, 0); индексы монет пула.
    Curve { i: i128, j: i128 },
    /// Пул DODO V2 (DVM / DPP / DSP): вход переводом, sellBase / sellQuote по _BASE_TOKEN_ пула.
    Dodo,
    /// WooFi WooPPV2: вход переводом, swap(from, to, сколько дошло, 0, получатель, исполнитель).
    Woo,
    /// Maverick V2: swap + maverickV2SwapCallback.
    Maverick,
    /// KyberSwap Elastic: swap(получатель, вход, isToken0, предел, data) + swapCallback.
    Elastic,
    /// PancakeSwap StableSwap и Lista: exchange(uint256 i, uint256 j, dx, 0); индексы монет пула.
    StableU256 { i: u8, j: u8 },
    /// Saddle (Nerve): swap(uint8 i, uint8 j, dx, 0, block.timestamp); индексы монет пула.
    Saddle { i: u8, j: u8 },
    /// PancakeSwap StableSwap / Lista, монета пары — нативный BNB (решение «122 X (нативный BNB в стейблсвапах)»):
    /// bnb_in — монета i нативная (вход: WBNB.withdraw → exchange{value}); иначе монета j (выход: BNB → WBNB.deposit).
    /// Токены шага — WBNB и встречная монета.
    StableU256Bnb { i: u8, j: u8, bnb_in: bool },
}

impl Params {
    /// Номер вида шага — константа контракта. Ошибок нет.
    pub fn kind(&self) -> u8 {
        match self {
            Params::V2 { .. } => 0,
            Params::V3 => 1,
            Params::Algebra => 2,
            Params::Solidly => 3,
            Params::Curve { .. } => 4,
            Params::Dodo => 5,
            Params::Woo => 6,
            Params::Maverick => 7,
            Params::Elastic => 8,
            Params::StableU256 { .. } => 9,
            Params::Saddle { .. } => 10,
            Params::StableU256Bnb { .. } => 11,
        }
    }

    /// Данные вида (поле Step.data). Ошибок нет.
    pub fn data(&self) -> Bytes {
        match *self {
            Params::V2 { fee_pips } => Bytes::from(fee_pips.abi_encode()),
            Params::Curve { i, j } => Bytes::from((i, j).abi_encode_params()),
            Params::StableU256 { i, j } => {
                Bytes::from((U256::from(i), U256::from(j)).abi_encode_params())
            }
            Params::Saddle { i, j } => {
                Bytes::from(<(Uint<8>, Uint<8>) as SolType>::abi_encode_params(&(i, j)))
            }
            Params::StableU256Bnb { i, j, bnb_in } => {
                Bytes::from((U256::from(i), U256::from(j), bnb_in).abi_encode_params())
            }
            _ => Bytes::new(),
        }
    }
}

/// Шаг маршрута до кодирования.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSpec {
    pub params: Params,
    /// Адрес пула (у каждого вида — свой контракт пула).
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    /// Доля текущего баланса исполнителя по token_in, 1…10000 (последний шаг по токену — ALL_BPS).
    pub share_bps: u16,
}

/// Кодирует шаг в тип ABI контракта. Принимает: шаг. Возвращает: Step. Ошибки (текст): доля вне 1…10000, токены входа и
/// выхода совпадают, токен шага StableSwap — метка нативной монеты 0xEeee… (не ERC-20: шаг с BNB пула — STABLE_U256_BNB с
/// токеном WBNB), нулевой адрес — контракт отверг бы такой шаг откатом, здесь — до отправки.
pub fn encode(s: &StepSpec) -> Result<ArbExecutor::Step, String> {
    if s.share_bps == 0 || s.share_bps > ALL_BPS {
        return Err(format!("доля шага {} bps вне 1…{ALL_BPS}", s.share_bps));
    }
    if s.token_in == s.token_out {
        return Err("у шага одинаковые токены входа и выхода".into());
    }
    if s.pool.is_zero() || s.token_in.is_zero() || s.token_out.is_zero() {
        return Err("у шага нулевой адрес пула или токена".into());
    }
    let stable = matches!(
        s.params,
        Params::Curve { .. }
            | Params::StableU256 { .. }
            | Params::Saddle { .. }
            | Params::StableU256Bnb { .. }
    );
    if stable && (s.token_in == STABLE_NATIVE || s.token_out == STABLE_NATIVE) {
        return Err(
            "токен шага StableSwap — метка нативной монеты 0xEeee… (не ERC-20): шаг через пул с BNB — STABLE_U256_BNB с токеном WBNB"
                .into(),
        );
    }
    if let Params::V2 { fee_pips } = s.params
        && fee_pips >= 1_000_000
    {
        return Err(format!("комиссия пары V2 {fee_pips} ≥ 1e6 долей"));
    }
    Ok(ArbExecutor::Step {
        kind: s.params.kind(),
        pool: s.pool,
        tokenIn: s.token_in,
        tokenOut: s.token_out,
        shareBps: s.share_bps,
        data: s.params.data(),
    })
}
