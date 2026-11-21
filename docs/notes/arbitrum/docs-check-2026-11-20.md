# Arbitrum One (42161): проверка по официальной документации (20.11.2026)

Источник: агент-исследование (WebFetch docs.arbitrum.io, релизы Nitro, страницы DEX). GeckoTerminal и CoinGecko недоступны (robots) — список DEX сети нужно записать с сервера: `GET /networks/arbitrum/dexes?page=N`.

## Газ
- Типы транзакций — как в Ethereum + свои 100–106. Ставка PGA — `maxPriorityFeePerGas` у EIP-1559 (type 2). Наш код строит только legacy (`src/venues/bsc_dex/send/tx.rs`) → для Arbitrum нужна сборка EIP-1559.
- Плата: baseFee L2 × (газ L2 + L1-calldata в газе L2). L1-часть (poster fee): Brotli-размер × 16 × цена байта L1 ÷ baseFee L2.
- Узнать заранее: `eth_estimateGas` уже включает L1; отдельно — NodeInterface `0xc8` `gasEstimateComponents` / `gasEstimateL1Component`. В квитанции — `gasUsedForL1`. Минимальная цена газа — `ArbGasInfo.getMinimumGasPrice`.
- Чаевые: min(maxPriority, maxFee − baseFee); на One собираются (97% DAO, 3% Developer Guild).
- Пределы: 32 Mgas на блок и на транзакцию, calldata до 95 000 байт.

## Отправка
- Порядок на One — PGA (раунды по 125 мс, 2 раунда на блок 250 мс; сортировка по чаевым, при равных — по времени прихода к секвенсору). Timeboost выключен.
- Мемпула нет, бандлов/билдеров нет → атомарность только одной транзакцией через свой исполнитель.
- Отправка: `https://arb1-sequencer.arbitrum.io/rpc` (только `eth_sendRawTransaction` и `eth_sendRawTransactionConditional`; условия knownAccounts / окна блока L1 / времени L2, ошибка −32003). Есть `eth_sendRawTransactionSync`.
- Сколько чаевых нужно, чтобы выигрывать, — в документации нет.

## Узел (Nitro)
- `eth_simulateV1` — нужен Nitro ≥ v3.12.1 (до него падал на цепях с чаевыми, Arbitrum One — такая).
- State override у `eth_call`, `eth_subscribe` logs/newHeads — в документации не описаны, проверять на своём узле.
- `debug` надо добавить в `--http.api`; WebSocket — `--ws.port/--ws.addr/--ws.origins`.
- Cancun (TSTORE/TLOAD, MCOPY) — с ArbOS 20 Atlas.
- `block.number` в контракте = номер блока L1.

## Блоки
- Блок 250 мс, мягкая финальность — менее секунды (секвенсор), жёсткая — после пакета на L1.

## GeckoTerminal
- Id сети `arbitrum`. У 72 из 139 DEX BSC id без метки сети (`pancakeswap_v2`, `thena`…) → ключ области «DEX» должен включать сеть.

## Покрытие DEX (по объёму за 30 дней, DefiLlama 17.11)
| DEX | Семейство | В коде | Примечание |
|---|---|---|---|
| Uniswap | V2/V3/V4 | есть | адреса из developers.uniswap.org |
| Aero | ? | Slipstream/Solidly есть, ABI не сверен | адресов нет |
| Fluid | Fluid DEX T1 | **нет** | новое семейство |
| Metric | MetricOmm (BSC) | версия на Arbitrum не проверена | адресов нет |
| PancakeSwap | V3 | есть | |
| WOOFi | WooPPV2 | есть | |
| GMX | ? | нет | не пул |
| Camelot | V2 (направленные комиссии, стабильные пары), V3 Algebra 1.9 (feeZtO/feeOtZ), V4 Algebra Integral | V4 есть; V2/V3 частично | |
| Curve | StableSwap / Twocrypto / Tricrypto | StableSwap есть; crypto **нет** | |
| DODO | DVM/DSP/DPP | есть | |
| Sushi | V2 / V3 | V2 есть, V3 не сверен | |
| Balancer | V2 / V3 | V2 есть; V3 **нет** | |
| LFJ | Liquidity Book | есть только v2.1 | версия на Arbitrum не проверена |
| Native, Dexalot | RFQ / книга | нет | не пулы |

## Нужны отдельные задачи
1. Fluid DEX. 2. Camelot V3 (направленные комиссии). 3. Camelot V2. 4. Aero (ABI, адреса). 5. Curve Twocrypto/Tricrypto. 6. Balancer V3. 7. LB v2.2 (если нужно). 8. Metric на Arbitrum. 9. Отправка EIP-1559 + чаевые PGA, `eth_sendRawTransactionConditional`. 10. Газ с L1-частью. 11. RFQ/книги (Native, Dexalot, GMX) — отдельный источник котировок.

## Имя сети Arbitrum у бирж (записанные ответы API)
BingX `ARBITRUM`, KuCoin `arbitrum`, Gate `ARBEVM`; остальные — по живым ответам.
