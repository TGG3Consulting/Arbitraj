# Arbitrum One — Liquidity Book v2.0 (LFJ V2), запись своего узла (решение 445)

Записано скриптом `tools/arbitrum_record/lb_v20.py` (только чтение; ключей, подписей и транзакций нет).

- Блок: 525986940, хеш 0x010e3c1acfedd5d01293955cbe8533f9e22095255a7c7b30a0da93d504560509, время 1795193481 (UTC 2026-11-20T16:51:21+00:00)
- eth_chainId: 0xa4b1
- Фабрика LBFactory v2.0: 0x1886D09C9Ade0c5DB822D85D21678Db67B6c2982; роутер LBRouter v2.0 (квотер getSwapOut): 0x7BFd7192E76D950832c77BB412aaE841049D8D9B
- Реализация пары: код 24380 байт, SHA-256 a485017692004321f23962332cb749a50786bb3c2bee26decd42b603c9500430 (сверка с исходником lfj-gg/joe-v2@v2.0.0 — вне бота)
- Предел бинов на сторону: 300; логи — 20000 блоков до записанного

## Файлы
- `meta.json` — блок, заголовок блока, eth_chainId, адреса, список пулов.
- `factory.json` — getNumberOfLBPairs, LBPairImplementation, код реализации, allLBPairs(i) и getReservesAndId при отборе.
- `pool_<адрес>.json` — factory/tokenX/tokenY/feeParameters/getReservesAndId, getLBPairInformation фабрики,
  decimals токенов, getBin активного бина, обход бинов (findFirstNonEmptyBinId + getBin; конец стороны —
  откат или предел), getSwapOut роутера на долях резерва в обе стороны (с откатами), слоты 6, 7, 10, код пула.
- `logs_<адрес>.json` — eth_getLogs пула (Swap, DepositedToBin, WithdrawnFromBin, CompositionFee).

## Пулы
- 0x13fda18516eafe5e8ae930f86fa51ae4b6c35e8f: X 0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9, Y 0xff970a61a04b1ca14834a43f5de4533ebddb5cc8, шаг 1, активный 8388600, бинов вниз 21 (revert), вверх 76 (revert), getSwapOut успешных 11 из 14
- 0x2d7d00424d7e61a198afda2109407d60199ecffe: X 0xda10009cbd5d07dd0cecc66161fc93d7c9000da1, Y 0xff970a61a04b1ca14834a43f5de4533ebddb5cc8, шаг 1, активный 8112283, бинов вниз 25 (revert), вверх 23 (revert), getSwapOut успешных 12 из 14
- 0x7ec3717f70894f6d9ba0be00774610394ce006ee: X 0x82af49447d8a07e3bd95bd0d56f35241523fbab1, Y 0xff970a61a04b1ca14834a43f5de4533ebddb5cc8, шаг 15, активный 8375114, бинов вниз 222 (revert), вверх 186 (revert), getSwapOut успешных 12 из 14
- 0x5fbbef48ce0850e5a73bc3f4a6e903458b3c0af4: X 0xfc5a1a6eb076a2c7ad06ed22c90d7e710e35ad0a, Y 0x82af49447d8a07e3bd95bd0d56f35241523fbab1, шаг 20, активный 8386862, бинов вниз 0 (revert), вверх 256 (revert), getSwapOut успешных 7 из 14
- 0x021352af332c70817e25a47578a8f0d8c0fa9210: X 0xfc5a1a6eb076a2c7ad06ed22c90d7e710e35ad0a, Y 0xff970a61a04b1ca14834a43f5de4533ebddb5cc8, шаг 30, активный 8380604, бинов вниз 0 (revert), вверх 64 (revert), getSwapOut успешных 7 из 14
