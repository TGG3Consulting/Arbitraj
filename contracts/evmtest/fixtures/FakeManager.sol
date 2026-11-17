// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/FakeManager.sol
// Что делает: ФИКСТУРА — поддельный «менеджер» шага UNI_V4 (решение 390, ревью t181 Н5): произвольный контракт в роли
//   PoolManager. unlock зовёт у вызывающего unlockCallback с ПОДЛИННЫМИ данными шага (их ему передал сам исполнитель),
//   swap / sync / settle / take ничего не делают, exttload ВРЁТ дельтами. Режимы:
//   0 — «долг = сумма шага, кредит огромный» (исполнитель переведёт долг, выхода не получит → откат «minOut»);
//   1 — «долг = сумма шага + 1» (откат «owed»);
//   2 — перед обратным вызовом пробует получить оплату через pancakeV3SwapCallback (устаревшие expToken / expMax прошлого
//       шага V3 той же транзакции); успех — откат «stale paid»; затем как режим 0.
//   В сети не разворачивается.
// Кто вызывает: contracts/evmtest/singleton.mjs.
// От чего зависит: подписи — contracts/IPools.sol (IV4PoolManager, ISingletonSettle).
// Настройки: нет.
pragma solidity 0.8.37;

struct FmKey {
    address currency0;
    address currency1;
    uint24 fee;
    int24 tickSpacing;
    address hooks;
}

struct FmSwap {
    bool zeroForOne;
    int256 amountSpecified;
    uint160 sqrtPriceLimitX96;
}

interface IFmExec {
    function unlockCallback(bytes calldata data) external returns (bytes memory);
    function pancakeV3SwapCallback(int256 d0, int256 d1, bytes calldata data) external;
}

contract FakeManager {
    uint8 public mode;
    address public tokenIn;
    address public tokenOut;
    uint256 public amount;

    // Настройка: режим и токены шага (по ним exttload отличает долг входа от кредита выхода).
    function set(uint8 m, address tin, address tout) external {
        (mode, tokenIn, tokenOut) = (m, tin, tout);
    }

    function unlock(bytes calldata data) external returns (bytes memory) {
        if (mode == 2) {
            (bool ok,) = msg.sender.call(abi.encodeCall(IFmExec.pancakeV3SwapCallback, (int256(1e18), int256(0), "")));
            require(!ok, "stale paid");
        }
        return IFmExec(msg.sender).unlockCallback(data);
    }

    function swap(FmKey memory, FmSwap memory p, bytes calldata) external returns (int256) {
        amount = uint256(-p.amountSpecified);
        return 0;
    }

    // Ложные дельты: по слоту keccak256(вызывающий, валюта) — как TransientStateLibrary.currencyDelta.
    function exttload(bytes32 slot) external view returns (bytes32) {
        if (slot == keccak256(abi.encode(msg.sender, tokenIn))) {
            int256 debt = -int256(mode == 1 ? amount + 1 : amount);
            return bytes32(uint256(debt));
        }
        if (slot == keccak256(abi.encode(msg.sender, tokenOut))) return bytes32(uint256(1e30));
        return bytes32(0);
    }

    function sync(address) external {}

    function settle() external payable returns (uint256) {
        return 0;
    }

    function take(address, address, uint256) external {}
}
