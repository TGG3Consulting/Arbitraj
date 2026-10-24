# tests/data/live_2026-10-24 — живой запуск Тиграна 2026-10-24

`dex_subscription.json`:
- `journal_bingx_no_pair` — запись журнала (SQLite `journal`, kind `venue.subscribe`) дословно: монеты, у которых у BingX нет торгуемой пары.
- `pools` — пулы PancakeSwap V3 «монета — USDT» из сырого журнала узла BSC (`data/raw/bsc_dex/1792839382198.jsonl.zst`,
  окно `log_window_ms`, ≈31 мин): адрес пула, число уведомлений `notifications` за окно, тикер BingX и адрес токена.
  Адрес токена получен сверкой CREATE2 (деплоер и POOL_INIT_CODE_HASH PancakeSwap V3 — поле `pancakeswap_v3`,
  из официального репозитория pancake-v3-contracts) — тест это перепроверяет.
- Тикер BITLIGHT — так BingX называет монету с этим адресом в сети BSC (по журналу того же запуска: «PancakeSwap V3 BITLIGHT»).

Ответ BingX о сетях монет (wallets/capital/config) в этой записи нет.
