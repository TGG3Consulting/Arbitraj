# Исполнитель ArbExecutor v4 (via-IR) на EVM — шаги UNI_V4 / INF_CL / INF_BIN (решение 457), 20.11.2026

`singleton_steps.json` пишет `contracts/evmtest/run.mjs` (сценарий G1 `contracts/evmtest/singleton.mjs`) — тот же сценарий
и тот же формат, что `tests/data/executor_v3_evm_2026-11-17/` (README там), но байткод — текущая сборка
`contracts/out/ArbExecutor.bin-runtime` = исполнитель v4, собранный конвейером via-IR (решение 457; solc 0.8.37,
оптимизатор 200 прогонов, SHA-256 bin-runtime 2bf86e58e5719f88d709bcf06350570cbd60318f1e49e9f9d3a9c85c3ee10762).
Запись v3 (executor_v3_evm_2026-11-17) с решения 457 не перезаписывается.

Отличие от записи v3 (факт сравнения файлов): у всех шести случаев совпадают ключ, `data`, `amount_in`, `got`,
`event_in`, `event_out`; отличается только `gas` вызова execute (v4: 122 212 / 122 456 / 134 296 / 134 456 / 106 901 /
107 025; v3: 121 181 / 121 426 / 133 409 / 133 570 / 106 127 / 106 254). Оценка газа развёртывания: `deploy_gas_v4`
4 810 229, `deploy_gas_v3` 4 448 990 (= записи v3), `deploy_gas_v2` 3 607 554.

Это не запись сети: пулы созданы в тесте (официальный код менеджеров из vendor-копий, фикстура SingletonLp.sol).
Тестами бота пока не читается (tests/bsc_dex_executor_v3.rs читает запись v3).
