# executor_v2_evm_2026-11-14 — исполнение executeNative (решение 272) на EVM ethereumjs

`native_settled.json` пишет `contracts/evmtest/run.mjs`: события (Deposit/Withdrawal WETH9, Transfer, Swap/Sync
PancakePair, NativeSettled исполнителя) настоящего байткода v2 (`contracts/out/ArbExecutor.bin-runtime`) на EVM
ethereumjs Cancun с официальными PancakePair / PancakeFactory (pancake-swap-core@3b214306) и WETH9
(v2-periphery@ed249913). Это не сеть BSC: адреса — адреса тестовой цепи, суммы — по резервам тестовой пары.
Случаи: `sell` (продажа 1 BNB), `sell_left` (доля 5000 bps — половина вернулась), `buy` (покупка на 1000 USDT),
`sell_foreign_bnb` (ревью 272 А3: как `sell_left`, но на исполнителе заранее 3 подкинутых BNB — `foreign_wei`; событие —
только свои операции). Байткод — после ревью 272 (обёртка в конструкторе, `executeNative` без параметра `wnative`).
Читает: tests/bsc_dex_executor_v2.rs, tests/bsc_dex_executor_v2_review.rs.
