//! Файл: venues/bsc_dex/node/abi.rs
//! Что делает: описания функций контрактов для ABI-кодирования (alloy sol!) — ERC-20.
//! Селекторы и разбор ответов считает alloy по канонической сигнатуре; описания пулов и роутеров
//! добавляются в своих файлах (pools/*, exec/*) со ссылками на исходники протоколов.
//! Кто вызывает: wallet/balances.rs, wallet/approve.rs, wallet/transfer.rs, check/*.
//! От чего зависит: alloy::sol_types.
//! Настройки: нет.
//!
//! ERC-20 — официальный стандарт EIP-20: https://eips.ethereum.org/EIPS/eip-20
//! (balanceOf, decimals, symbol, transfer, approve, allowance; событие Transfer).

alloy::sol! {
    /// ERC-20 (EIP-20, раздел «Methods» и «Events»).
    interface IERC20 {
        function balanceOf(address owner) external view returns (uint256 balance);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
        function allowance(address owner, address spender) external view returns (uint256 remaining);
        function transfer(address to, uint256 value) external returns (bool success);
        function approve(address spender, uint256 value) external returns (bool success);
        event Transfer(address indexed from, address indexed to, uint256 value);
    }
}
