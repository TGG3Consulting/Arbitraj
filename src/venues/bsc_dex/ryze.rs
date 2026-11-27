//! Спотовый AMM Ryze. EIP‑712 и выплаты собственному кошельку сверяются с исполненной заявкой в batch.
pub(crate) mod book;
pub(crate) mod chain;
pub(crate) mod deployment;
pub(crate) mod execute;
pub mod fill;
pub mod http;
pub mod ledger;
pub(crate) mod market;
pub mod price;
mod preflight;
pub(crate) mod recovery;
pub(crate) mod routing;
mod result;
mod other;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod relay_tests;
#[cfg(test)]
mod price_tests;
pub(crate) mod tokens;
pub mod wire;
