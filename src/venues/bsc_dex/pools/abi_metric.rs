//! Файл: venues/bsc_dex/pools/abi_metric.rs
//! Что делает: ABI пула Metric OMM (решение 285, DEX metric-bsc) и его фабрики — только вызовы, которые бот делает:
//! getImmutables (проверка пула: фабрика, токены — verify_metric.rs), getSellAndBuyPrices (пригодна ли цена провайдера
//! на блоке — load_metric.rs), swap (обмен — шаг METRIC исполнителя, виртуальный квотер), isPool фабрики (реестр).
//! Обратный вызов, который пул зовёт у вызывающего, — metricOmmSwapCallback(int256,int256,bytes) (0xc3251075).
//! Кто вызывает: pools/verify_metric.rs, pools/load_metric.rs, pools/events_metric.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет (фабрика — dex.factory [DEX]).
//!
//! Источник (решение 23; исходник пулов BSC НЕ опубликован — ни Sourcify, ни репозитория; проверено 15.11.2026):
//! ABI — официальный SDK команды Metric (npm @nradko/metric-omm-sdk-v1@0.4.5, репозиторий
//! github.com/Metric-OMM/metric-sdk; src/abis/MetricOmmPool.ts, MetricOmmPoolFactory.ts, MetricOmmSimpleRouter.ts —
//! metricOmmSwapCallback). Совпадение с развёрнутым кодом BSC — ФАКТОМ цепи (tests/data/metric_2026-11-15):
//! в коде пула есть PUSH4 селекторов swap 0xe24f0884, getSellAndBuyPrices 0xec069057, getImmutables 0xbcdb4dad и
//! PUSH32 селектора metricOmmSwapCallback 0xc3251075; getImmutables всех 385 пулов фабрики отвечает 24 словами (как
//! кортеж SDK v1); у фабрики 0x2a53833c… есть isPool 0x5b16ebb7, nextPoolIdx, idxToPool, poolToIdx. ОТЛИЧИЯ BSC от SDK:
//! simulateSwapAndRevert на BSC — 0x658b6cb7 (в SDK 0x5cced60b — подпись другая; бот его не зовёт), createPool другой;
//! topic0 событий пула BSC в SDK нет (имена не установлены) — берутся из записанных логов (topics()).
//! Решение 482 (Metric V1 на Arbitrum): исходник фабрики 0x2a53…8f32 опубликован в Sourcify 42161 (match;
//! lib/metric-core/contracts/MetricOmmPoolFactory.sol): nextPoolIdx — L101 (= 1 в конструкторе, L146), idxToPool —
//! L104, createPool — L285-L287 (номер пула = nextPoolIdx++, idxToPool[номер] = пул): пулы фабрики — номера
//! 1..nextPoolIdx − 1 (перечисление — pools/metric_list.rs); событие PoolCreated — interfaces/IMetricOmmPoolFactory
//! (адрес пула и оба токена — indexed; подписка — created_log.rs). Код фабрики BSC по тому же адресу исходника не имеет
//! (createPool другой, см. выше) — событие BSC может не совпасть: тогда подписка на BSC просто не срабатывает.

// Код функций генерирует alloy::sol! — число полей задаёт ABI контракта, а не мы.
#![allow(clippy::too_many_arguments)]

use alloy_primitives::{B256, b256};

alloy::sol! {
    /// Неизменяемые параметры пула (кортеж getImmutables SDK v1, 24 поля).
    struct MetricImmutables {
        address factory;
        address token0;
        address token1;
        uint256 token0ScaleMultiplier;
        uint256 token1ScaleMultiplier;
        uint256 initialScaledToken0PerShareE18;
        uint256 initialScaledToken1PerShareE18;
        uint256 minimalMintableLiquidity;
        address immutablePriceProvider;
        int256 lowestBin;
        int256 highestBin;
        address extension1;
        address extension2;
        address extension3;
        address extension4;
        address extension5;
        address extension6;
        address extension7;
        uint256 beforeAddLiquidityOrder;
        uint256 afterAddLiquidityOrder;
        uint256 beforeRemoveLiquidityOrder;
        uint256 afterRemoveLiquidityOrder;
        uint256 beforeSwapOrder;
        uint256 afterSwapOrder;
    }

    interface IMetricOmmPool {
        function getImmutables() external view returns (MetricImmutables memory);
        function getSellAndBuyPrices() external returns (uint128 sellPriceX64, uint128 buyPriceX64);
        function swap(address recipient, bool zeroForOne, int128 amountSpecified, uint128 priceLimitX64, bytes callbackData, bytes extensionData) external returns (int128, int128);
    }

    /// Решение 482: порядок срабатываний расширений (struct ExtensionOrders — PoolExtensionsConfig.sol, Sourcify 42161
    /// MetricOmmPoolFactory) — нужен только для подписи события PoolCreated.
    struct ExtensionOrders {
        uint256 beforeAddLiquidity;
        uint256 afterAddLiquidity;
        uint256 beforeRemoveLiquidity;
        uint256 afterRemoveLiquidity;
        uint256 beforeSwap;
        uint256 afterSwap;
    }

    interface IMetricOmmFactory {
        function isPool(address pool) external view returns (bool);
        function nextPoolIdx() external view returns (uint256);
        function idxToPool(uint256 idx) external view returns (address);
        event PoolCreated(address indexed poolAddress, address indexed token0, address indexed token1, uint256 poolIdx, address factory, address admin, address priceProvider, address[] extensions, ExtensionOrders extensionOrders, uint256 priceProviderTimelock, uint256 initialScaledAmount0PerE18Shares, uint256 initialScaledAmount1PerE18Shares, uint256 minimalOperationalLiquidity, uint24 spreadProtocolFeeE6, uint24 protocolNotionalFeeE8, uint24 adminSpreadFeeE6, uint24 adminNotionalFeeE8, address adminFeeDestination, int24 curBinDistFromProvidedPriceE6, uint256[] nonNegativeBinDataArray, uint256[] negativeBinDataArray);
    }
}

/// topic0 событий пула из записанных логов BSC (tests/data/metric_2026-11-15/probe.json, «logs»: пулы WBNB/USDT и
/// NVDAB/USDT, блоки 133653733…133656733; имена событий не установлены — исходник BSC не опубликован). Любое из них →
/// перечитать пул; цена провайдера и так меняется каждым блоком (per_block.rs). Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![
        // 1 тема, 3 слова данных — на каждом обмене (299 из 592 логов).
        b256!("0xcad09799f755048f0fd388c6b2fcc8e0cebefd15756c6903522cc2a95f661f70"),
        // 3 темы (две индексированные), 3 слова — на каждом обмене (291 из 592).
        b256!("0x9734819749a91fc3be03ea83205f924ee08479bd3f0da48efc91d94d050cac1e"),
        // 3 темы, 58 и 22 слова — по одному логу (что это за события — не установлено).
        b256!("0xf53d04bdbda6d88dd536895ec533833d3e09d0cbe2474fae32a368c5f83fc610"),
        b256!("0xa55772434bee2a347743b1e8086fa44bac69a060e124ec92ab6b9cc7fc5268af"),
    ]
}

/// topic0 для мелких пулов (pools/group.rs Topics::Liquidity). Ревью t117: какое из 4 событий — изменение ликвидности,
/// не установлено (исходника нет) — мелкие пулы слушают все 4 (без догадки). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    topics()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    /// Селекторы ABI SDK = селекторам, найденным в коде пула и фабрики BSC (PUSH4 / PUSH32, probe.json).
    #[test]
    fn selectors_match_chain() {
        assert_eq!(IMetricOmmPool::swapCall::SELECTOR, [0xe2, 0x4f, 0x08, 0x84]);
        assert_eq!(
            IMetricOmmPool::getSellAndBuyPricesCall::SELECTOR,
            [0xec, 0x06, 0x90, 0x57]
        );
        assert_eq!(
            IMetricOmmPool::getImmutablesCall::SELECTOR,
            [0xbc, 0xdb, 0x4d, 0xad]
        );
        assert_eq!(
            IMetricOmmFactory::isPoolCall::SELECTOR,
            [0x5b, 0x16, 0xeb, 0xb7]
        );
        assert_eq!(liquidity_topics(), topics());
    }
}
