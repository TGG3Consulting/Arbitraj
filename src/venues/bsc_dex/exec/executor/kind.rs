//! Файл: venues/bsc_dex/exec/executor/kind.rs
//! Что делает: ПУБЛИЧНЫЙ кодировщик шагов исполнителя ArbExecutor (решение 122, часть X) — один вход для всех семейств
//! DEX: вид шага с его параметрами (Params), пул, токены, доля входа (StepSpec) → шаг ABI контракта (abi::ArbExecutor::Step).
//! Часть F (новые семейства: Curve, WooFi, Maverick…) строит шаги только через `encode` — ABI и числа видов здесь.
//! Номера видов — константы контракта contracts/ArbExecutor.sol (V2 = 0 … ELASTIC = 8, STABLE_U256 = 9, SADDLE = 10,
//! STABLE_U256_BNB = 11, NOMI_STABLE = 12 — только v2, решение 284; METRIC = 13 — только v2, решение 285; IZI = 14 — только v2, решение 286; KYBER_DMM = 15 — только v2, решение 287; SMARDEX = 16 — только v2, решение 288; BALANCER = 17 — только v2, решение 289; LB = 18 — только v2, решение 290; UNI_V4 = 19, INF_CL = 20, INF_BIN = 21 — только v3, решение 395 (t181); FLUID = 22 — только v4, решение 440; BALANCER_V3 = 23, LB20 = 24, EKUBO = 25 — только v4, решения 441, 445, 447, 456; сверяет тест tests/bsc_dex_executor_abi.rs с
//! исходником); раскладка data — комментарий Step в
//! контракте: V2 — abi.encode(uint32 feePips), CURVE — (int128 i, int128 j), STABLE_U256 — (uint256 i, uint256 j),
//! SADDLE — (uint8 i, uint8 j), STABLE_U256_BNB — (uint256 i, uint256 j, bool bnbIn), METRIC — (bool zeroForOne),
//! BALANCER — (bytes32 poolId), UNI_V4 — abi.encode(PoolKey Uniswap V4), INF_CL / INF_BIN — abi.encode(PoolKey
//! Infinity) (кодирование ключа — pools/key.rs, то же, что даёт PoolId), FLUID — (bool swap0to1, uint8 native, address
//! liquidity), остальные — пусто.
//! Решение 441: BALANCER_V3 = 23 — только v4, data — abi.encode(address пула).
//! Кто вызывает: exec/executor/{call, entry}.rs; часть F (новые семейства).
//! От чего зависит: exec/executor/abi.rs, alloy sol-types.
//! Настройки: нет.

use alloy::sol_types::sol_data::Uint;
use alloy::sol_types::{SolType, SolValue};
use alloy_primitives::{Address, B256, Bytes, U256};

use super::super::super::pools::stable_native;
use super::abi::ArbExecutor;
pub use super::kind_key::{InfKey, V4Key};

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
    /// Пара Nomiswap Stable (решение 284; только исполнитель v2): вход переводом, выход — getAmountOut(tokenIn, вход)
    /// самой пары, swap как у V2. Данных нет.
    NomiStable,
    /// Пул Metric OMM (решение 285; только исполнитель v2): swap + metricOmmSwapCallback; zero_for_one — направление в
    /// порядке токенов САМОГО пула (getImmutables), data = abi.encode(bool).
    Metric { zero_for_one: bool },
    /// Пул iZiSwap / Biswap V3 (решение 286; только исполнитель v2): swapX2Y / swapY2X по порядку адресов токенов шага
    /// + обратный вызов. Данных нет.
    Izi,
    /// Пул KyberSwap Classic DMM (решение 287; только исполнитель v2): вход переводом в пул, выход — по getTradeInfo() пула
    /// (формула DMMLibrary.getAmountOut в контракте), swap как у V2. Данных нет.
    KyberDmm,
    /// Пара SmarDex (решение 288; только исполнитель v2): swap(to, zeroForOne по порядку адресов, +вход, «») +
    /// smardexSwapCallback. Данных нет.
    Smardex,
    /// Пул Balancer V2 (решение 289; только исполнитель v2): пул шага — хранилище Vault, approve ровно на шаг +
    /// Vault.swap(GIVEN_IN) по id пула; data = abi.encode(bytes32 poolId).
    Balancer { pool_id: B256 },
    /// Пул Liquidity Book V2.1 (решение 290; только исполнитель v2): вход переводом в пул, swap(swapForY, to);
    /// направление контракт берёт по getTokenX() пула (порядок X/Y — создателя пула, не адреса). Данных нет.
    Lb,
    /// Решение 441 (T433-F2; только исполнитель v4): пул Balancer V3 — пул шага хранилище Vault V3 (dex.vault),
    /// data = abi.encode(address пула); обмен — unlock → balancerV3Callback → swap / settle / sendTo.
    BalancerV3 { pool: Address },
    /// Пул Liquidity Book v2.0 (решение 445): вход переводом в пул, swap(swapForY, to); направление контракт берёт по
    /// tokenX() пары v2.0 (getTokenX у v2.0 нет). Данных нет. Шаг 24 — в исходнике contracts/ArbExecutor.sol; в коде
    /// исполнителя — только сборка v4 (pools/exec_callback.rs since; сборки v4 ещё нет).
    Lb20,
    /// Решение 395 (t181; только исполнитель v3): пул Uniswap V4 внутри PoolManager — ключ пула; пул шага — PoolManager.
    UniV4 { key: V4Key },
    /// Решение 395 (только v3): пул PancakeSwap Infinity CL (и Infinity Stable — пул в крючке CL) — ключ пула (менеджер —
    /// в ключе); пул шага — Vault Infinity (dex.vault).
    InfCl { key: InfKey },
    /// Решение 395 (только v3): пул PancakeSwap Infinity Bin — ключ пула; пул шага — Vault Infinity (dex.vault).
    InfBin { key: InfKey },
    /// Решение 447: пул Ekubo в ядре Core — config ключа пула (токены ключа — токены шага по порядку адресов); пул шага —
    /// ядро. Шаг есть только у кода исполнителя с EKUBO (pools/exec_callback.rs). Решение 454: data = abi.encode(bytes32
    /// config, bool native, bool forward): native — token0 ключа нативный ETH (сторона шага в обёртке wnative
    /// исполнителя разворачивается), forward — обмен через Core.forward(расширение) (только MEVCapture; TWAMM, Oracle,
    /// BoostedFees — обычный swap ядра, их beforeSwap зовёт само ядро).
    Ekubo {
        config: B256,
        native: bool,
        forward: bool,
    },
    /// Решение 440 (только исполнитель v4): пул Fluid DEX T1 — zero_for_one в порядке токенов САМОГО пула (swap0to1),
    /// native: 0 — нативной монеты на шаге нет, 1 — вход в нативной монете пула (токен шага — обёртка: withdraw →
    /// swapIn{value}), 2 — выход в нативной монете (приходит от слоя ликвидности → deposit в обёртку); liquidity — слой
    /// ликвидности пула (constantsView), только от него контракт принимает нативную монету на шаге.
    /// data = abi.encode(bool, uint8, address).
    Fluid(FluidStep),
}

/// Решение 440: параметры шага FLUID (см. Params::Fluid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidStep {
    pub zero_for_one: bool,
    /// 0 — нет нативной монеты, 1 — вход в ней, 2 — выход в ней.
    pub native: u8,
    pub liquidity: Address,
}

impl FluidStep {
    /// abi.encode(bool zeroForOne, uint8 native, address liquidity) — uint8 занимает слово, как uint256. Ошибок нет.
    pub fn data(&self) -> Bytes {
        let t = (self.zero_for_one, U256::from(self.native), self.liquidity);
        Bytes::from(t.abi_encode_params())
    }
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
            Params::NomiStable => 12,
            Params::Metric { .. } => 13,
            Params::Izi => 14,
            Params::KyberDmm => 15,
            Params::Smardex => 16,
            Params::Balancer { .. } => 17,
            Params::Lb => 18,
            Params::UniV4 { .. } => 19,
            Params::InfCl { .. } => 20,
            Params::InfBin { .. } => 21,
            Params::Fluid(_) => 22,
            // Решение 441: номер 23 = 21 + номер семейства T433-F2 (22 — за F1), чтобы шаги семейств не совпали.
            Params::BalancerV3 { .. } => 23,
            // Решение 456: LB20 = 24 (в ветке t223-f6 было 22 — перенумеровано при слиянии).
            Params::Lb20 => 24,
            // Решение 456: EKUBO = 25 (в ветке t230 было 22 — перенумеровано при слиянии).
            Params::Ekubo { .. } => 25,
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
            Params::Metric { zero_for_one } => Bytes::from(zero_for_one.abi_encode()),
            Params::Balancer { pool_id } => Bytes::from(pool_id.abi_encode()),
            Params::BalancerV3 { pool } => Bytes::from(pool.abi_encode()),
            Params::Ekubo {
                config,
                native,
                forward,
            } => Bytes::from((config, native, forward).abi_encode_params()),
            // Решение 395: abi.encode(ключ) — статическая структура, слова подряд (как PoolId — keccak тех же байт).
            Params::UniV4 { key } => Bytes::from(key.pool_key().abi()),
            Params::InfCl { key } | Params::InfBin { key } => Bytes::from(key.pool_key().abi()),
            Params::Fluid(f) => f.data(),
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
/// токеном WBNB), нулевой адрес, у шага пула внутри менеджера (решение 395) токены не валюты ключа или нативная валюта в
/// ключе — контракт отверг бы такой шаг откатом, здесь — до отправки.
pub fn encode(s: &StepSpec) -> Result<ArbExecutor::Step, String> {
    if s.share_bps == 0 || s.share_bps > ALL_BPS {
        return Err(format!("доля шага {} bps вне 1…{ALL_BPS}", s.share_bps));
    }
    if s.token_in == s.token_out {
        return Err("у шага одинаковые токены входа и выхода".into());
    }
    // Решение 395: шаг пула внутри менеджера — токены шага ровно валюты ключа, обе ERC-20 (иначе контракт:
    // «key» / «native»);
    // до проверки нулевого адреса — у пула с нативной валютой причина точнее.
    match s.params {
        Params::UniV4 { key } => {
            super::kind_key::check_tokens((key.currency0, key.currency1), s.token_in, s.token_out)?
        }
        Params::InfCl { key } | Params::InfBin { key } => {
            super::kind_key::check_tokens((key.currency0, key.currency1), s.token_in, s.token_out)?
        }
        _ => {}
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
    // Ревью 440: у шага FLUID токены — ERC-20 (нативная монета пула — обёрткой, native в данных шага).
    let fluid = matches!(s.params, Params::Fluid(_));
    if fluid && (s.token_in == STABLE_NATIVE || s.token_out == STABLE_NATIVE) {
        return Err("токен шага FLUID — метка 0xEeee… (не ERC-20): токен шага — обёртка".into());
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
