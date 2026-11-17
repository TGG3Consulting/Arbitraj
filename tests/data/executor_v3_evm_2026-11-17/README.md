# Исполнитель ArbExecutor v3 на EVM — шаги UNI_V4 / INF_CL / INF_BIN (решение 395, t181), 17.11.2026

`singleton_steps.json` пишет `contracts/evmtest/run.mjs` (сценарий G1 `contracts/evmtest/singleton.mjs`): EVM ethereumjs
(Cancun), байткод v3 из `contracts/out/ArbExecutor.bin-runtime` и ОФИЦИАЛЬНЫЙ код менеджеров, собранный в тесте solc 0.8.26
(via_ir, cancun) из vendor-копий без изменений — Uniswap/v4-core@bea7f6d1 `PoolManager.sol`, pancakeswap/infinity-core@3c5ef1be
`Vault.sol`, `pool-cl/CLPoolManager.sol`, `pool-bin/BinPoolManager.sol`. Это не запись сети BSC: пулы созданы в тесте
(токены — официальный ERC20 pancake-swap-core, ликвидность — фикстура SingletonLp.sol).

Поля случая: `name` (вид), `kind` (номер вида), `pool` (PoolManager / Vault), `token_in`, `token_out`, `key` (поля ключа
пула по порядку PoolKey: V4 — currency0, currency1, fee, tickSpacing, hooks; Infinity — currency0, currency1, hooks,
poolManager, fee, parameters), `data` (байты Step.data, которые контракт принял и исполнил), `amount_in`, `got` (прирост
выхода у владельца), `event_in` / `event_out` (суммы события Swap менеджера), `gas` (газ вызова execute в EVM).
Поля `deploy_gas_v3` / `deploy_gas_v2` — оценка газа транзакции развёртывания (исполнение + внутренняя цена создания).

Используется: tests/bsc_dex_executor_v3.rs (данные шага бота = байты, принятые контрактом).
