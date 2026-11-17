// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/SingletonLp.sol
// Что делает: ФИКСТУРА-поставщик ликвидности для EVM-теста шагов UNI_V4 / INF_CL / INF_BIN (решение 395, t181). Сами
//   менеджеры — официальный код (vendor/v4-core PoolManager, vendor/infinity-core Vault / CLPoolManager / BinPoolManager);
//   здесь только «положить ликвидность»: открыть менеджер (unlock / lock), внутри — modifyLiquidity / mint, затем погасить
//   дельты этого контракта (sync → перевод → settle; кредит — take себе), как DeltaResolver официальной периферии.
//   Токены поставщика — на его балансе (LP переводит заранее). В обменах исполнителя не участвует.
// Кто вызывает: contracts/evmtest/singleton.mjs.
// От чего зависит: подписи — vendor/v4-core/src/interfaces/IPoolManager.sol (unlock, modifyLiquidity, sync, settle,
//   take, exttload), vendor/infinity-core/src/{Vault.sol, interfaces/IVault.sol, pool-cl/interfaces/ICLPoolManager.sol,
//   pool-bin/interfaces/IBinPoolManager.sol}.
// Настройки: нет.
pragma solidity 0.8.37;

struct LpV4Key {
    address currency0;
    address currency1;
    uint24 fee;
    int24 tickSpacing;
    address hooks;
}

struct LpInfKey {
    address currency0;
    address currency1;
    address hooks;
    address poolManager;
    uint24 fee;
    bytes32 parameters;
}

struct LpRange {
    int24 tickLower;
    int24 tickUpper;
    int256 liquidityDelta;
    bytes32 salt;
}

struct LpBinMint {
    bytes32[] liquidityConfigs;
    bytes32 amountIn;
    bytes32 salt;
}

interface ILpV4 {
    function unlock(bytes calldata data) external returns (bytes memory);
    function modifyLiquidity(LpV4Key memory key, LpRange memory params, bytes calldata hookData)
        external
        returns (int256, int256);
    function exttload(bytes32 slot) external view returns (bytes32);
}

interface ILpVault {
    function lock(bytes calldata data) external returns (bytes memory);
    function currencyDelta(address settler, address currency) external view returns (int256);
}

interface ILpCl {
    function modifyLiquidity(LpInfKey memory key, LpRange memory params, bytes calldata hookData)
        external
        returns (int256, int256);
}

interface ILpBin {
    function mint(LpInfKey memory key, LpBinMint calldata params, bytes calldata hookData) external returns (int256);
}

interface ILpSettle {
    function sync(address currency) external;
    function settle() external payable returns (uint256);
    function take(address currency, address to, uint256 amount) external;
}

interface ILpErc20 {
    function transfer(address to, uint256 value) external returns (bool);
}

contract SingletonLp {
    // Позиция Uniswap V4: менеджер, ключ, диапазон и ликвидность.
    function addV4(address pm, LpV4Key calldata key, LpRange calldata r) external {
        ILpV4(pm).unlock(abi.encode(uint8(0), abi.encode(key, r)));
    }

    // Позиция Infinity CL: Vault, ключ (poolManager — CLPoolManager), диапазон.
    function addCl(address vault, LpInfKey calldata key, LpRange calldata r) external {
        ILpVault(vault).lock(abi.encode(uint8(1), abi.encode(key, r)));
    }

    // Ликвидность Infinity Bin: Vault, ключ (poolManager — BinPoolManager), раскладка по бинам.
    function addBin(address vault, LpInfKey calldata key, LpBinMint calldata m) external {
        ILpVault(vault).lock(abi.encode(uint8(2), abi.encode(key, m)));
    }

    function unlockCallback(bytes calldata data) external returns (bytes memory) {
        (, bytes memory body) = abi.decode(data, (uint8, bytes));
        (LpV4Key memory key, LpRange memory r) = abi.decode(body, (LpV4Key, LpRange));
        ILpV4(msg.sender).modifyLiquidity(key, r, "");
        _square(msg.sender, key.currency0, true);
        _square(msg.sender, key.currency1, true);
        return "";
    }

    function lockAcquired(bytes calldata data) external returns (bytes memory) {
        (uint8 mode, bytes memory body) = abi.decode(data, (uint8, bytes));
        LpInfKey memory key;
        if (mode == 1) {
            LpRange memory r;
            (key, r) = abi.decode(body, (LpInfKey, LpRange));
            ILpCl(key.poolManager).modifyLiquidity(key, r, "");
        } else {
            LpBinMint memory m;
            (key, m) = abi.decode(body, (LpInfKey, LpBinMint));
            ILpBin(key.poolManager).mint(key, m, "");
        }
        _square(msg.sender, key.currency0, false);
        _square(msg.sender, key.currency1, false);
        return "";
    }

    // Погасить дельту поставщика по валюте: долг — sync → перевод → settle; кредит — take себе.
    function _square(address m, address c, bool uni) internal {
        int256 d = uni
            ? int256(uint256(ILpV4(m).exttload(keccak256(abi.encode(address(this), c)))))
            : ILpVault(m).currencyDelta(address(this), c);
        if (d < 0) {
            ILpSettle(m).sync(c);
            require(ILpErc20(c).transfer(m, uint256(-d)), "lp transfer");
            ILpSettle(m).settle();
        } else if (d > 0) {
            ILpSettle(m).take(c, address(this), uint256(d));
        }
    }
}
