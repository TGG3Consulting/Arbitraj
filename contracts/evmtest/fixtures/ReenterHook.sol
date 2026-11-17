// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/ReenterHook.sol
// Что делает: ФИКСТУРА-крючок Uniswap V4 с флагом BEFORE_SWAP (адрес с битом 1 << 7 — vendor/v4-core Hooks.sol L38):
//   в beforeSwap пытается повторно войти в исполнитель — unlockCallback, lockAcquired и uniswapV3SwapCallback с
//   «правдоподобными» данными. Любой успешный повторный вход — откат «reentered» (обмен не пройдёт); все отказали —
//   обмен идёт как обычно (нулевая дельта крючка, комиссия пула без изменения). Проверка решения 395: обратный вызов
//   менеджера исполняется только один раз, только от менеджера шага и только с данными шага.
// Кто вызывает: официальный PoolManager (vendor/v4-core) при swap пула с этим крючком; адрес и цель задаёт singleton.mjs.
// От чего зависит: подпись beforeSwap — vendor/v4-core/src/interfaces/IHooks.sol (BeforeSwapDelta = int256, lpFeeOverride
//   uint24); ключ и параметры — те же поля, что PoolKey / SwapParams v4-core.
// Настройки: нет.
pragma solidity 0.8.37;

struct HookKey {
    address currency0;
    address currency1;
    uint24 fee;
    int24 tickSpacing;
    address hooks;
}

struct HookSwap {
    bool zeroForOne;
    int256 amountSpecified;
    uint160 sqrtPriceLimitX96;
}

contract ReenterHook {
    // Исполнитель, в который пробуем войти (слот 0 — задаёт тест через setTarget; код крючка копируется на адрес с
    // флагом, хранилище — своё у адреса).
    address public target;
    // Данные, которые подставим в обратный вызов (тест кладёт ровно данные шага исполнителя).
    bytes public data;
    uint256 public tries;

    function setTarget(address t, bytes calldata d) external {
        target = t;
        data = d;
    }

    function beforeSwap(address, HookKey calldata, HookSwap calldata, bytes calldata)
        external
        returns (bytes4, int256, uint24)
    {
        bytes[3] memory calls = [
            abi.encodeWithSignature("unlockCallback(bytes)", data),
            abi.encodeWithSignature("lockAcquired(bytes)", data),
            abi.encodeWithSignature("uniswapV3SwapCallback(int256,int256,bytes)", int256(1), int256(0), bytes(""))
        ];
        for (uint256 i = 0; i < calls.length; ++i) {
            (bool ok,) = target.call(calls[i]);
            require(!ok, "reentered");
            ++tries;
        }
        return (this.beforeSwap.selector, 0, 0);
    }
}
