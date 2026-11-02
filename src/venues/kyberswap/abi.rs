//! Файл: venues/kyberswap/abi.rs
//! Что делает: описания функций контракта ERC-20 для ABI-кодирования (alloy sol!) — своя копия адаптера kyberswap
//! (изоляция: не импортирует bsc_dex). Нужны для баланса, разрешения (approve/allowance) и перевода монеты.
//! Кто вызывает: balances.rs, trade.rs, transfer.rs.
//! От чего зависит: alloy::sol_types.
//! Настройки: нет.
//!
//! ERC-20 — официальный стандарт EIP-20: https://eips.ethereum.org/EIPS/eip-20
//! (balanceOf, decimals, allowance, approve, transfer; событие Transfer).

alloy::sol! {
    /// ERC-20 (EIP-20, разделы «Methods» и «Events»).
    interface IERC20 {
        function balanceOf(address owner) external view returns (uint256 balance);
        function decimals() external view returns (uint8);
        function allowance(address owner, address spender) external view returns (uint256 remaining);
        function approve(address spender, uint256 value) external returns (bool success);
        function transfer(address to, uint256 value) external returns (bool success);
        event Transfer(address indexed from, address indexed to, uint256 value);
    }
}
