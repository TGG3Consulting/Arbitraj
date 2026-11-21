// SPDX-License-Identifier: MIT
// Файл: contracts/evmtest/fixtures/EkuboQuoter.sol
// Что делает: ФИКСТУРА-квотер для EVM-теста шага EKUBO (решение 458, T457-01). Ядро и MEVCapture — официальный код
//   (байткод развёртывания Arbitrum из broadcast EkuboProtocol/evm-contracts, vendor/ekubo). Router.quote официального
//   репозитория в сети Arbitrum не развёрнут (в broadcast 42161 его нет), собрать его нельзя (solc 0.8.33 нет в
//   package-lock) — здесь повторён ТОЛЬКО его путь (src/Router.sol L419-L447 + BaseLocker.lockAndExpectRevert
//   L82-L116 @61d09daa): lock ядра → в locked_6416899205 обмен на точный вход — swap_6269342730 ядра (ключ 3 слова +
//   SwapParameters) или forward(расширение) с abi.encode(ключ, параметры) (как MEVCaptureRouter._swap) → откат с
//   ответом (balanceUpdate, stateAfter); quote ловит откат и возвращает balanceUpdate. Состояние не меняется.
// Кто вызывает: contracts/evmtest/ekubo.mjs.
// От чего зависит: формат вызовов ядра — FlashAccountant.sol L146-L221, Core.sol L510-L525 (@61d09daa).
// Настройки: нет.
pragma solidity 0.8.37;

contract EkuboQuoter {
    address internal core;

    // Котировка: ядро, токены ключа, config, SwapParameters (bytes32), forward-расширение (0 — обычный swap ядра).
    // Возвращает: balanceUpdate (delta0 << 128 | delta1). Ошибки: откат ядра / расширения пробрасывается.
    function quote(address core_, address t0, address t1, bytes32 config, bytes32 params, address fwd)
        external
        returns (bytes32 balanceUpdate)
    {
        core = core_;
        (bool ok, bytes memory r) = core_.call(abi.encodePacked(bytes4(0xf83d08ba), abi.encode(t0, t1, config, params, fwd)));
        require(!ok && r.length == 68 && bytes4(r) == bytes4(keccak256("Quote(bytes32,bytes32)")), _why(r));
        assembly {
            balanceUpdate := mload(add(r, 36))
        }
    }

    // Обратный вызов ядра (селектор 0x00000000): обмен и откат с ответом.
    fallback() external {
        require(msg.sender == core, "core");
        (address t0, address t1, bytes32 config, bytes32 params, address fwd) =
            abi.decode(msg.data[36:], (address, address, bytes32, bytes32, address));
        (bool ok, bytes memory r) = fwd == address(0)
            ? core.call(abi.encodePacked(bytes4(0), abi.encode(t0, t1, config), params))
            : core.call(abi.encodePacked(bytes4(0x101e8952), abi.encode(fwd, t0, t1, config, params)));
        if (!ok) {
            assembly {
                revert(add(r, 32), mload(r))
            }
        }
        (bytes32 bu, bytes32 st) = abi.decode(r, (bytes32, bytes32));
        bytes memory q = abi.encodeWithSelector(bytes4(keccak256("Quote(bytes32,bytes32)")), bu, st);
        assembly {
            revert(add(q, 32), mload(q))
        }
    }

    function _why(bytes memory r) internal pure returns (string memory) {
        return r.length == 0 ? "quote: empty" : "quote: unexpected";
    }
}
