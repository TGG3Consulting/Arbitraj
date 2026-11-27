# Прямой Hashflow RFQT в четырёх сетях

Записаны публичные транзакции, которые уже исполнились и истекли. В каждой записи `eth_getTransactionByHash` даёт адрес роутера, отправителя, value и фактическую calldata. `eth_getTransactionReceipt` даёт статус, газ и лог пула. Хеш квитанции и хеш транзакционного блока сверены с `eth_getBlockByNumber` по высоте. В `swaps.json` сохранены только логи Trade и необходимые поля. Время истечения подписанной котировки проверяется относительно времени добытого блока.

| Сеть | Chain ID | Блок | Транзакция |
|---|---:|---:|---|
| Ethereum | 1 | 26439388 | [Etherscan](https://etherscan.io/tx/0xafe6f9081a9bb73249d74c5c035de39d4696f69f20f1115aea7acda8a9bbd147) |
| BSC | 56 | 85821758 | [BscScan](https://bscscan.com/tx/0x345c676646b88894e4bde88466461ec474f56bcf4f8325453be2c25da83644d9) |
| Base | 8453 | 30560603 | [BaseScan](https://basescan.org/tx/0xc285ebd7544a1f98920fd63edaf02f7d27c0a2a0d6943fcb3571a4aea687d72c) |
| Arbitrum One | 42161 | 441395898 | [Arbiscan](https://arbiscan.io/tx/0xd8ccf4630a66a1605eeee5b590c4f0571131cee7769d568ad1fce50a461c6cce) |

Официальные [адреса роутеров](https://github.com/hashflownetwork/x-protocol/blob/main/evm/deployed-contracts/IHashflowRouter.json), [ABI роутера](https://github.com/hashflownetwork/x-protocol/blob/main/evm/abi/IHashflowRouter.json) и [ABI пула](https://github.com/hashflownetwork/x-protocol/blob/main/evm/abi/IHashflowPool.json) определяют источник и формат проверки. Живые ответы `price-levels` и `rfq` требуют доступа владельца. В этой записи представлены только результаты исторических сделок других участников.

## Контроль обычной сделки и восстановления

`amm_controls.json` содержит отдельный принятый swap через пул Uniswap V3 или PancakeSwap V3 на каждой из четырёх сетей, адреса, исход сделки и логи передачи обеих валют. `scripts/record_hashflow_amm_controls.py` повторно сверяет принадлежность пула фабрике, decimals, каноничность блока и хеш транзакции. `scripts/record_hashflow_swaps.py` сверяет архивные прямые транзакции Hashflow и логи ERC-20. Вызовы выполняются через JSON-RPC для чтения опубликованных данных.

Rust-проверки `hashflow::recovery_tests` используют пустую конфигурацию RPC, закрытый vault и SQLite. При выключенном Hashflow в каждой из четырёх сетей обычная котировка сохраняется, разбор квитанции даёт те же объём, среднюю цену и комиссию. Для каждой архивной RFQ отдельная проверка сначала восстанавливает зарегистрированный хеш и nonce после потери памяти процесса, затем подтверждает прежний торговый результат по реальной квитанции. Новая сетевая отправка для восстановления не нужна.

## Проверка рынка на 27 ноября 2026 года

В интервале 05:00–06:00 UTC скрипт `scripts/record_hashflow_markets.py` прочитал все логи с темой события Hashflow `Trade`. Для каждого эмитента он проверил в конце интервала обе связи — официальный роутер разрешает пул через `authorizedPools`, а пул возвращает тот же адрес из `router()`. В архиве `markets_hour.json` сохранены границы по времени и блокам, исходные слова событий, хеши кода и результаты публичных запросов. [Закреплённый реестр Hashflow](https://github.com/hashflownetwork/x-protocol/blob/e41cfaadac7d9bacfa368be904a93bd98b5a2df6/evm/deployed-contracts/IHashflowRouter.json)

| Сеть | Включённые блоки | Trade за час | Направленные пары |
|---|---|---:|---:|
| Ethereum | 26 504 311–26 504 609 | 11 | 6 |
| BSC | 135 978 845–135 986 842 | 0 | 0 |
| Base | 62 826 122–62 844 121 | 0 | 0 |
| Arbitrum One | 527 917 584–527 929 289 | 0 | 0 |

Четыре пула из `swaps.json` оставались разрешёнными на проверенных высотах. Счётчики относятся к этому событию и часовому интервалу. Доступные сейчас конкретному кошельку пары, объёмы и окончательные цены устанавливаются через авторизованные `market-makers`, `price-levels` и `rfq`. Публичный запрос списка без заголовка авторизации вернул HTTP 403 и HTML на всех четырёх сетях. В режиме Multi-Wallet укажите выданное Hashflow имя источника в `hashflow.source` и соответствующий ключ в `hashflow.taker_key`. Для индивидуального Taker укажите в `hashflow.source` значение `api`, внесите выданный ключ в тот же секрет и публичный адрес его владельца в `hashflow.api_wallet`. Список маркетмейкеров и уровни запрашиваются с параметром `wallet`, а для RFQ используется `trader`. Исторические данные показывают исполнение для своих исходных кошельков. [Порядок доступа и форма заявки](https://docs.hashflow.com/hashflow/taker/getting-started-api-v3)
