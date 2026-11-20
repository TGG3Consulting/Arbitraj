# executor_v3_live_2026-11-17 — как запись лежит в репозитории (t185, решение 400)

Запись прислал серверный ИИ архивом `executor_v3_live_2026-11-17.tar.gz` (SHA-256 архива
`fa8d6ff0fd7677e8b30d67561581dbcc08b6978562d2a01307621ab54a924eb6`). Файлы записи не редактировались:

- `README.md`, `summary.json`, `hooks_reject.json`, `calls.json`, `slots.json` — как в архиве;
- `raw_rpc.json` (7 658 202 байт, SHA-256 `5b11829bdd2d1cfc928adab13a27227cd5453a37f9f72f9b44d5156092d0b152`) — сжат zstd
  без изменений в `raw_rpc.json.zst` (как прочие большие записи tests/data, например dodo_kjudge_2026-11-16_2);
- `scripts/*.py` — скрипты прогона серверного ИИ, только справка (что делалось); в репозитории не запускаются.

Бот читает `README.md` (SHA-256 симулированного кода), `summary.json` и `hooks_reject.json`
(src/venues/bsc_dex/exec/executor/live_proof.rs, вшиты include_str). Тест tests/bsc_dex_executor_v3_live.rs сверяет
summary.json с calls.json (статус, возврат execute, кредит события Swap менеджера) и raw_rpc.json.zst (код исполнителя в
stateOverrides = contracts/out/ArbExecutor.bin-runtime; квотер в той же симуляции сразу перед (б)). Правило приёмки —
docs/DECISIONS.md, решение 400.

## Сеть записи (решение 461)

В сыром `README.md` строки chainId нет (сырые файлы не правятся). Сеть записи — BSC, по факту сырого README: строка
«WBNB (dex.native_wrapped, bsc_dex): 0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c» и адреса пулов BSC; запись — со
своего узла BSC. Бот читает сеть записи из строки ниже (src/venues/bsc_dex/exec/executor/live_record.rs, `chain_in`):

- chainId: 56
