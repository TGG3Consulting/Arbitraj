//! Файл: venues/bsc_dex/exec/executor/abi.rs
//! Что делает: ABI контракта-исполнителя ArbExecutor (решение 122, часть X) — типы вызова execute, шага Step и
//! обратных вызовов генерируются alloy `sol!` прямо из артефакта сборки contracts/out/ArbExecutor.abi (ABI JSON, solc).
//! Одна точка правды: поменялся контракт → contracts/build.sh → новый ABI → Rust-типы меняются при компиляции.
//! Кто вызывает: exec/executor/{kind, call, sim, trade}.rs, тесты (tests/bsc_dex_executor_abi.rs).
//! От чего зависит: alloy::sol! (функция json), contracts/out/ArbExecutor.abi.
//! Настройки: нет.

alloy::sol!(
    #[derive(Debug, PartialEq, Eq)]
    ArbExecutor,
    "contracts/out/ArbExecutor.abi"
);
