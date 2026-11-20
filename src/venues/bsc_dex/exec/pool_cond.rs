//! Файл: venues/bsc_dex/exec/pool_cond.rs
//! Что делает: решение 438 — что ставить в условие knownAccounts (eth_sendRawTransactionConditional, секвенсор
//! Arbitrum) по каждому пулу маршрута цикла DEX ↔ DEX: слоты хранилища (Slots), корень хранилища контракта пула (Root)
//! или без условия с причиной (Off). Значения читает exec/pool_cond_read.rs на блоке плана.
//! Решение 439: условие пулов ставится только при send.pool_cond_mode = pools (send/pool_cond_mode.rs wanted); не
//! задана или off — правила не применяются, значения пулов не читаются, условная отправка — только с окном времени.
//! Кто вызывает: exec/pool_cond_read.rs.
//! От чего зависит: pools/{registry, families}.rs (вид, ссылка на пул), core/types (PoolRef).
//! Настройки: нет (режим send.pool_cond_mode читает send/pool_cond_mode.rs wanted).
//!
//! Как узел сверяет условие (nitro-geth 260bc5dcbd5a16925d7e26145577b2612f30f428 arbitrum_types/txoptions.go:79-107):
//! у адреса — либо корень хранилища statedb.GetStorageRoot (:92-97), либо каждый слот statedb.GetState целым словом
//! 32 байта (:98-104); пустой список слотов — «нет условия» (:104). Поэтому у одного адреса — одно из двух; слоты пулов
//! одного менеджера (Uniswap V4) сливаются в один список.
//!
//! Таблица видов (правило раскладки Solidity: переменные — по порядку объявления от базового контракта к наследнику,
//! мелкие поля пакуются в слот, структура и mapping начинают новый слот, constant/immutable слотов не занимают):
//!
//! | Вид | Адрес | Слоты | Источник (репозиторий, коммит, файл:строка) |
//! | --- | --- | --- | --- |
//! | V2 (uniswap_v2, pancakeswap_v2) | пул | 8 — reserve0, reserve1, blockTimestampLast | Uniswap/v2-core 6a9e7c97 contracts/UniswapV2ERC20.sol:12-19 (totalSupply 0, balanceOf 1, allowance 2, DOMAIN_SEPARATOR 3, nonces 4), contracts/UniswapV2Pair.sol:18-24 (factory 5, token0 6, token1 7, резервы 8); pancakeswap/pancake-swap-core 3b214306 PancakeERC20.sol:12-19, PancakePair.sol:18-24 — та же раскладка |
//! | V2-форки (v2_fork, v2_fork_executor) | пул | корень | раскладка форков не сверялась |
//! | UniswapV3 (uniswap_v3) | пул | 0 — slot0; 4 — liquidity | Uniswap/v3-core 5ea68a77 contracts/UniswapV3Pool.sol:42-54 (immutable), 56-74 (Slot0: 160+24+16+16+16+8+8 = 248 бит → слот 0), 77 (1), 79 (2), 87 (protocolFees 3), 90 (liquidity 4); NoDelegateCall.sol:8 — immutable |
//! | PancakeV3 (pancakeswap_v3) | пул | 0 — slot0 (цена, тик); 5 — liquidity | pancakeswap/pancake-v3-contracts 98684794 projects/v3-core/contracts/PancakeV3Pool.sol:60-78 (160+24+16+16+16 = 232; uint32 feeProtocol не влезает → слот 1 с unlocked), 81 (2), 83 (3), 91 (4), 94 (liquidity 5) |
//! | V3-форки (v3_fork, v3_fork_pancake) | пул | корень | раскладка форков не сверялась |
//! | Slipstream | пул | 6 — slot0; 16 — liquidity + maxLiquidityPerTick | velodrome-finance/slipstream 193a6b85 contracts/core/CLPool.sol:41-51 (factory 0 … factoryRegistry 5), 53-69 (slot0 6: 240 бит), 72-95 (7…14), 98-102 (15), 105-107 (16) |
//! | KyberElastic | пул | 3 — poolData (sqrtP, тики, locked); 4 — baseL, reinvestL | KyberNetwork/ks-elastic-sc 4ab08c0a contracts/PoolStorage.sol:21-32 (PoolData), 62-68 (immutable), 70 (ticks 0), 71 (1), 73 (2), 75 (poolData 3, 4, …); ERC20 токена reinvest — после (Pool.sol:26, правая база) |
//! | LbV21 | пул | 4 — _parameters (activeId, изменчивость); 5 — _reserves | lfj-gg/joe-v2 v2.1.0 8eeb42d7 src/LBToken.sol:21-31 (0-2), src/libraries/ReentrancyGuard.sol:30 (3), src/LBPair.sol:31 (порядок баз), 58-63 (_parameters 4, _reserves 5) |
//! | LbV22 (решение 446) | пул | 3 — _parameters; 4 — _reserves; 65545 — _hooksParameters (ревью 446: смена крючков пула ловится условием) | lfj-gg/joe-v2 v2.2.0 1297c38 src/LBToken.sol:21-31 (0-2), src/libraries/ReentrancyGuardUpgradeable.sol:44-50 и OpenZeppelin contracts-upgradeable b40cdf32 Initializable.sol:65-77 (свои слоты ERC-7201 — обычных не занимают), src/LBPair.sol:33 (порядок баз), 63-73 (_parameters 3, _reserves 4, _protocolFees 5, _bins 6, _tree 7-9 — TreeMath.sol:15-19 bytes32 + 2 mapping, _oracle 10..65544 — OracleHelper.sol:31-33 bytes32[65535], _hooksParameters 65545); на узле не сверено — запись tools/arbitrum_record/lb_v22.py, самопроверка exec/pool_cond_check.rs |
//! | UniswapV4 | PoolManager | S = keccak256(PoolId . 6): S — slot0 (цена, тик, комиссии), S+3 — liquidity | Uniswap/v4-core bea7f6d1 src/libraries/StateLibrary.sol:11 (POOLS_SLOT 6), 19 (LIQUIDITY_OFFSET 3), 324-325 (слот пула); src/libraries/Pool.sol:83-91 |
//! | BalancerV2 | — | без условия | балансы в общем Vault: слоты посчитаны только вручную по AST (/home/claude/src-ext/reports/arb-known-accounts.md, 3.1), не сверены — docs/TASKS.md «Решение 438» |
//! | WooPP, пары общего контракта (PoolRef::Member) | — | без условия | общий контракт на все монеты, слоты не опубликованы |
//! | InfinityCl, InfinityBin, InfinityStable | — | без условия | виды только BSC (в Arbitrum нет), слоты менеджера/крючка не сверены |
//! | прочие виды со своим контрактом на пул (Algebra, AlgebraIntegral, Solidly*, Curve*, PancakeStable, Lista, Saddle, Nomi, DODO, KyberDmm, Smardex, Maverick, iZi, BiswapV3, MetricOmm) | пул | корень (eth_getProof … storageHash) | раскладка не подтверждена (Algebra Integral: версия Camelot V4 не сверена; Vyper, закрытые исходники) |
//!
//! Что ловится, что нет (V3 и форки, V4, Slipstream, Kyber Elastic): любой обмен пишет slot0 (цена) — ловится каждый
//! обмен конкурента; mint/burn в текущем диапазоне пишут liquidity — ловятся (v3-core UniswapV3Pool.sol:336-351).
//! НЕ ловятся mint/burn вне текущего диапазона: они пишут только ticks и tickBitmap (UniswapV3Pool.sol:328-335) — цена
//! и ликвидность в точке не меняются, а наш обмен они меняют, только если дойдёт до этих тиков; это не действие
//! арбитражника, а провайдера ликвидности, итог хуже плана откатит minOut исполнителя (платно). Тики и битмап пути в
//! условие не ставятся — вопрос в docs/TASKS.md («Решение 438»). V4: состояние крючка (hook) не ловится.
//! V2: любой swap/mint/burn/sync идёт через _update и пишет слот 8 (UniswapV2Pair.sol:73-82). LB: обмен пишет
//! _parameters и _reserves, mint/burn — _reserves (LBPair.sol:546-550, 608, 652, 712); затухание комиссии по времени
//! блока — без записи, не ловится. Корень: ловит любую запись в хранилище пула, в том числе не влияющую на цену
//! (переводы LP-токена, collect, наблюдения) — такой отказ бесплатный, но сделка теряется.

use alloy_primitives::{Address, B256, U256, keccak256};

use crate::core::types::PoolRef;

use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;

/// Что ставить в условие для одного пула.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// Слоты контракта по адресу (значения — eth_getStorageAt на блоке плана) и что это за слоты (для журнала).
    Slots(Address, Vec<B256>, &'static str),
    /// Корень хранилища контракта (eth_getProof … storageHash на блоке плана).
    Root(Address),
    /// Без условия: причина.
    Off(&'static str),
}

/// Слот пула Uniswap V4 в PoolManager: keccak256(abi.encodePacked(PoolId, POOLS_SLOT = 6)) (v4-core bea7f6d1
/// src/libraries/StateLibrary.sol:11, 324-325). Ошибок нет.
pub fn v4_state_slot(id: B256) -> B256 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(id.as_slice());
    buf[32..].copy_from_slice(&U256::from(6u8).to_be_bytes::<32>());
    keccak256(buf)
}

/// Слот со смещением (S + n). Ошибок нет (переполнение по модулю 2^256, как в EVM).
fn plus(s: B256, n: u8) -> B256 {
    let v = U256::from_be_bytes(s.0).wrapping_add(U256::from(n));
    B256::from(v.to_be_bytes::<32>())
}

/// Номер слота как слово. Ошибок нет.
fn slot(n: u32) -> B256 {
    B256::from(U256::from(n).to_be_bytes::<32>())
}

/// Что ставить в условие по пулу маршрута (таблица в шапке). Принимает: запись реестра. Ошибок нет.
pub fn rule(e: &Entry) -> How {
    let f = e.family;
    let own = |a: Address, slots: &[u32], what: &'static str| {
        How::Slots(a, slots.iter().map(|n| slot(*n)).collect(), what)
    };
    match (&e.pool.reference, f.proto) {
        (_, Proto::BalancerV2) => How::Off(
            "Balancer V2: балансы пула в общем Vault, слоты Vault не сверены (docs/TASKS.md, решение 438)",
        ),
        (_, Proto::InfinityCl | Proto::InfinityBin | Proto::InfinityStable) => How::Off(
            "PancakeSwap Infinity: вид только BSC, слоты менеджера и крючка не сверены",
        ),
        (PoolRef::Singleton { manager, id }, Proto::UniswapV4) => {
            let s = v4_state_slot(*id);
            How::Slots(*manager, vec![s, plus(s, 3)], "slot0 + liquidity (PoolManager)")
        }
        (PoolRef::Singleton { .. } | PoolRef::Member { .. }, _) => How::Off(
            "пул внутри общего контракта: слоты пула не подтверждены, корень общий для всех пулов",
        ),
        (PoolRef::Contract(a), Proto::V2) if !f.fork => own(*a, &[8], "резервы (слот 8)"),
        (PoolRef::Contract(a), Proto::UniswapV3) if !f.fork => {
            own(*a, &[0, 4], "slot0 + liquidity (слоты 0, 4)")
        }
        (PoolRef::Contract(a), Proto::PancakeV3) if !f.fork => {
            own(*a, &[0, 5], "slot0 + liquidity (слоты 0, 5)")
        }
        (PoolRef::Contract(a), Proto::Slipstream) => {
            own(*a, &[6, 16], "slot0 + liquidity (слоты 6, 16)")
        }
        (PoolRef::Contract(a), Proto::KyberElastic) => {
            own(*a, &[3, 4], "poolData + baseL/reinvestL (слоты 3, 4)")
        }
        (PoolRef::Contract(a), Proto::LbV21) => {
            own(*a, &[4, 5], "_parameters + _reserves (слоты 4, 5)")
        }
        (PoolRef::Contract(a), Proto::LbV22) => {
            own(
                *a,
                &[3, 4, 65545],
                "_parameters + _reserves + _hooksParameters (слоты 3, 4, 65545)",
            )
        }
        (PoolRef::Contract(a), _) => How::Root(*a),
        (PoolRef::Multi { contract, .. }, _) => How::Root(*contract),
    }
}
