//! Файл: venues/bsc_dex/pools/abi_lb22.rs
//! Что делает: решение 446 — то, что у пула Liquidity Book V2.2 есть сверх V2.1 (вид lb_v22; LFJ V2.2 на Arbitrum One):
//! implementation() (адрес реализации, которую исполняет клон пула), getLBHooksParameters() (крючки пула: адрес и флаги)
//! и событие HooksParametersSet; getReserves() (есть и у V2.1, в abi_lb.rs не нужен был) — для сверки слова _reserves
//! в тесте условия пулов. Остальное ABI — общее с V2.1 (abi_lb.rs: сигнатуры функций и событий в V2.2 не
//! менялись — docs LFJ «V2.2 Key Changes»: «existing functions have signatures unchanged»).
//! Кто вызывает: pools/lb_hooks.rs, pools/verify_lb22.rs, pools/events_lb22.rs.
//! От чего зависит: alloy::sol!.
//! Настройки: нет.
//!
//! Источник (решение 23): lfj-gg/joe-v2@v2.2.0 (1297c38) — src/interfaces/ILBPair.sol L68 (HooksParametersSet), L94
//! (implementation), L127 (getLBHooksParameters), L104 (getReserves); src/LBPair.sol L60, L81 (implementation — immutable
//! address(this) реализации), L247-L249 (getLBHooksParameters), L846-L861 (setHooksParameters: запись и событие — только фабрика).

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

alloy::sol! {
    interface ILBPair22 {
        function implementation() external view returns (address);
        function getLBHooksParameters() external view returns (bytes32 hooksParameters);
        function getReserves() external view returns (uint128 reserveX, uint128 reserveY);

        event HooksParametersSet(address indexed sender, bytes32 hooksParameters);
    }
}

/// topic0 события HooksParametersSet (ILBPair.sol L68). Ошибок нет.
pub fn hooks_set_topic() -> B256 {
    ILBPair22::HooksParametersSet::SIGNATURE_HASH
}
