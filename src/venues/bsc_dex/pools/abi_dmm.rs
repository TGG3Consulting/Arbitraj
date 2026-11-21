//! Файл: venues/bsc_dex/pools/abi_dmm.rs
//! Что делает: ABI KyberSwap Classic DMM (решение 287, вид kyber_dmm): пул — getTradeInfo (реальные и виртуальные резервы
//! и комиссия блока), события (Sync с виртуальными резервами, Swap, Mint, Burn, UpdateEMA); фабрика — getPools(a, b) (у
//! пары может быть НЕСКОЛЬКО пулов с разным усилением), событие PoolCreated; роутер DMMRouter02 — getAmountsOut по
//! пути пулов (квотер сверки перед сделкой). Поля пула factory/token0/token1 — как у V2 (abi_check.rs), обмен
//! swap(amount0Out, amount1Out, to, data) — та же подпись и селектор 0x022c0d9f, что у пары V2 (abi_v2v3.rs).
//! Решение 450 (T448-04): тот же вид — пулы KSPool фабрик Static Fee (KSFactory): код пула = DMMPool без
//! VolumeTrendRecorder / FeeFomula — getTradeInfo с постоянной feeInPrecision, та же проверка swap (verifyBalance), те же
//! события Sync / Swap / Mint / Burn, реестр getPools / isPool (dmm-smart-contracts@e557b57 KSPool.sol, KSFactory.sol
//! L131-L162, KSLibrary.sol = DMMLibrary); другое только событие создания пула (created_topics / created) и обратный
//! вызов ksSwapCall (исполнитель передаёт пустые данные — вызова нет). Квотер сверки — KSRouter02 своей фабрики.
//! Кто вызывает: pools/load_dmm.rs, events_dmm.rs, verify_calls.rs, verify_rules.rs, factory_ask.rs, created_log.rs,
//! quote/quoter_dmm.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (фабрика — dex.factory, роутер — dex.quoter [DEX]).
//!
//! Источник (решение 23): github.com/KyberNetwork/dmm-smart-contracts@e557b57 — contracts/DMMPool.sol (события L50-L61,
//! swap L162-L210, getTradeInfo L240-L265), contracts/DMMFactory.sol (PoolCreated L22-L28, getPools L94-L105, isPool
//! L119-L125), contracts/VolumeTrendRecorder.sol (UpdateEMA L24), contracts/periphery/DMMRouter02.sol (getAmountsOut
//! L625-L632). Развёрнуто в BSC: фабрика 0x878dfe971d44e9122048308301f540910bbd934c — Sourcify match (DMMFactory.sol,
//! solc 0.6.12, в том же файле — DMMPool, который фабрика создаёт `new DMMPool()`); код пула, VolumeTrendRecorder,
//! FeeFomula, MathExt и фабрики совпал с репозиторием без пробелов, кроме _mintFee пула (доля протокола при mint/burn — на
//! обмен не влияет); роутер 0x78df70615ffc8066cc0887917f2cd72092c86409 — Sourcify match DMMRouter02, factory() = фабрика;
//! адреса — официальная документация docs.kyberswap.com (Classic Contract Addresses, BSC, Dynamic Fee).

// Код функций и событий генерирует alloy::sol! — число аргументов задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, LogData};

alloy::sol! {
    interface IDmmPool {
        function getTradeInfo() external view returns (uint112 _reserve0, uint112 _reserve1, uint112 _vReserve0, uint112 _vReserve1, uint256 feeInPrecision);
        function ampBps() external view returns (uint32);

        event Mint(address indexed sender, uint256 amount0, uint256 amount1);
        event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
        event Swap(address indexed sender, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out, address indexed to, uint256 feeInPrecision);
        event Sync(uint256 vReserve0, uint256 vReserve1, uint256 reserve0, uint256 reserve1);
        event UpdateEMA(uint256 shortEMA, uint256 longEMA, uint128 lastBlockVolume, uint256 skipBlock);
    }

    interface IDmmFactory {
        function getPools(address token0, address token1) external view returns (address[] memory _tokenPools);
        function isPool(address token0, address token1, address pool) external view returns (bool);

        event PoolCreated(address indexed token0, address indexed token1, address pool, uint32 ampBps, uint256 totalPool);
    }

    /// Решение 450 (T448-04): KyberSwap Classic Static Fee — KSFactory (dmm-smart-contracts@e557b57 KSFactory.sol L26-L33;
    /// Arbitrum 0x1c758aF0…be5 — Sourcify 42161 match, solc 0.6.12, тот же текст): PoolCreated с feeUnits.
    interface IKsFactory {
        event PoolCreated(address indexed token0, address indexed token1, address pool, uint32 ampBps, uint24 feeUnits, uint256 totalPool);
    }

    /// Решение 450: KyberSwap Classic Legacy Static Fee — KSFactory Arbitrum 0x51E8D106…1a62 (Sourcify 42161 match, solc
    /// 0.6.12): PoolCreated с uint16 feeBps (в репозитории этой версии нет — текст события из верифицированного кода).
    interface IKsLegacyFactory {
        event PoolCreated(address indexed token0, address indexed token1, address pool, uint32 ampBps, uint16 feeBps, uint256 totalPool);
    }

    interface IDmmRouter {
        function getAmountsOut(uint256 amountIn, address[] calldata poolsPath, address[] calldata path) external view returns (uint256[] memory amounts);
    }
}

/// Решение 450: topic0 событий создания пула фабрик вида kyber_dmm — DMMFactory, KSFactory (Static Fee) и KSFactory
/// Legacy: у фабрики ключа событие одного из них (по её коду). Ошибок нет.
pub fn created_topics() -> [B256; 3] {
    [
        IDmmFactory::PoolCreated::SIGNATURE_HASH,
        IKsFactory::PoolCreated::SIGNATURE_HASH,
        IKsLegacyFactory::PoolCreated::SIGNATURE_HASH,
    ]
}

/// Решение 450: событие создания пула любой из трёх фабрик → (token0, token1, пул). Принимает: topic0, данные лога.
/// Возвращает: тройку или текст «не по ABI / чужое событие». Ошибок нет.
pub fn created(t0: B256, log: &LogData) -> Result<(Address, Address, Address), String> {
    let bad =
        |e: alloy::sol_types::Error| format!("PoolCreated (KyberSwap Classic) не по ABI: {e}");
    if t0 == IDmmFactory::PoolCreated::SIGNATURE_HASH {
        let e = IDmmFactory::PoolCreated::decode_log_data(log).map_err(bad)?;
        Ok((e.token0, e.token1, e.pool))
    } else if t0 == IKsFactory::PoolCreated::SIGNATURE_HASH {
        let e = IKsFactory::PoolCreated::decode_log_data(log).map_err(bad)?;
        Ok((e.token0, e.token1, e.pool))
    } else if t0 == IKsLegacyFactory::PoolCreated::SIGNATURE_HASH {
        let e = IKsLegacyFactory::PoolCreated::decode_log_data(log).map_err(bad)?;
        Ok((e.token0, e.token1, e.pool))
    } else {
        Err(format!("{t0} — не PoolCreated фабрик KyberSwap Classic"))
    }
}

/// topic0 событий изменения ликвидности (Mint, Burn). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![
        IDmmPool::Mint::SIGNATURE_HASH,
        IDmmPool::Burn::SIGNATURE_HASH,
    ]
}

/// topic0 всех событий пула, после которых он перечитывается (Sync, Swap, UpdateEMA, ликвидность). Ошибок нет.
pub fn topics() -> Vec<B256> {
    let mut out = vec![
        IDmmPool::Sync::SIGNATURE_HASH,
        IDmmPool::Swap::SIGNATURE_HASH,
        IDmmPool::UpdateEMA::SIGNATURE_HASH,
    ];
    out.extend(liquidity_topics());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;
    use alloy_primitives::keccak256;

    /// Селекторы и topic0 — keccak256 подписей исходника; селекторы getTradeInfo / getPools совпадают с PUSH4 в коде BSC.
    #[test]
    fn signatures_match_official() {
        assert_eq!(
            IDmmPool::getTradeInfoCall::SELECTOR,
            [0xd6, 0x69, 0x40, 0x27]
        );
        assert_eq!(
            IDmmFactory::getPoolsCall::SELECTOR,
            [0x5b, 0x1d, 0xc8, 0x6f]
        );
        assert_eq!(
            IDmmRouter::getAmountsOutCall::SELECTOR,
            [0xa8, 0x31, 0x2b, 0x1d]
        );
        assert_eq!(
            IDmmPool::Sync::SIGNATURE_HASH,
            B256::from(keccak256(b"Sync(uint256,uint256,uint256,uint256)"))
        );
        assert_eq!(
            IDmmFactory::PoolCreated::SIGNATURE_HASH,
            B256::from(keccak256(
                b"PoolCreated(address,address,address,uint32,uint256)"
            ))
        );
    }
}
