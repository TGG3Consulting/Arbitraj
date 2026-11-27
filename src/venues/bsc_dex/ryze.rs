//! Спотовый AMM Ryze. EIP‑712 и выплаты собственному кошельку сверяются с исполненной заявкой в batch.
pub mod fill;
#[cfg(test)]
mod tests;
pub mod wire;
