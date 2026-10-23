# venues/bsc_dex — адаптер DEX в сети BSC

Одна площадка `bsc_dex`: один кошелёк бота, один узел BSC; внутри — DEX (PancakeSwap, Uniswap), виды пулов
и каналы отправки (ARCHITECTURE.md, раздел «Раскладка адаптера DEX»). Реализует контракт `gateway/contract.rs`.
Всё — только по официальной документации протоколов и данным самого блокчейна (CLAUDE.md, решение 23).

## Официальные источники

| Источник | Что там |
| --- | --- |
| research/flow/bsc_node.md | Узлы и провайдеры BSC, chainId 56, блок 0,45 с |
| research/flow/bsc_dex.md | PancakeSwap/Uniswap V2 и V3: адреса, события, математика, исполнение |
| research/flow/bsc_v4.md | PancakeSwap Infinity (CL, Bin), Uniswap V4: PoolKey, Initialize, крючки, математика |
| research/flow/bsc_send.md, bsc_builders.md | Отправка транзакций: открытая, MEV Guard, билдеры, трекер |
| research/flow/bsc_smart_router.md | Умный режим: Universal Router PancakeSwap Infinity |
| https://github.com/ethereum/execution-apis (коммит 16caee27) | Методы JSON-RPC eth_*, eth_subscribe |
| https://www.jsonrpc.org/specification | Формат JSON-RPC 2.0, ошибки, пачки |
| https://github.com/bnb-chain/bsc (v1.8.1, 947ba1cb) | Тексты ошибок узла, EIP-155 по RPC, лимиты подписок |
| https://eips.ethereum.org/EIPS/eip-20, eip-155, eip-2718 | ERC-20, подпись с chainId, кодирование транзакции |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue`; функции следующих шагов Issue #7 честно отвечают «ещё не сделано» |
| `ctx.rs` | Окружение: настройки (площадка, DEX, канал), секреты, HTTP, журнал, сырые сообщения; `{key}` в адресе |
| `errors.rs` | Ответы узла (JSON-RPC, HTTP, сеть) → единые коды шлюза |
| `num.rs` | Суммы блокчейна U256 ↔ Decimal с decimals токена |
| `tokens.rs` | Монеты из подписки ядра (адрес — из API биржи) и их decimals из контракта |
| `connection.rs` | «Проверить подключение»: узел HTTP, узел WebSocket (newHeads), кошелёк (адрес, BNB) |
| `node/` | Связь с узлом: JSON-RPC по HTTP и WebSocket, подписки, головы сети, eth_call (свой README) |
| `wallet/` | Кошелёк: подпись, строгая очередь nonce, балансы (свой README) |

## Как устроена связь с узлом

- Транспорт свой (reqwest и tokio-tungstenite, как у адаптера BingX), типы и подпись — alloy:
  ответы JSON-RPC (`alloy::rpc::json_rpc`), ABI (`alloy::sol!`), транзакции и подпись (`alloy::consensus`, `alloy::signers`).
  Так сырые сообщения узла пишутся на диск (`storage/raw_log`), а таймауты, тишина и пауза переподключения
  берутся из настроек и меняются на лету.
- HTTP — запросы (eth_call, балансы, nonce); WebSocket — только подписки (newHeads, позже logs).
- Подписки привязаны к соединению: обрыв закрывает их все; владелец подписки переподключается после
  `venue.ws_reconnect_pause_ms` и перечитывает состояние (данные до этого — недостоверны).
- Ключ провайдера в адресе — как `{key}`, сам ключ — секрет `bsc.node_ws_key` / `bsc.node_http_key`;
  адрес с ключом не попадает ни в журнал, ни в текст ошибок.

## Настройки, которые читает шаг 1a

`bsc.node_http_url`, `bsc.node_ws_url`, `http.timeout_ms` (ответ узла и подключение), `venue.ws_silence_ms` [bsc_dex],
`venue.ws_reconnect_pause_ms` [bsc_dex], `feed.stale_after_ms` [bsc_dex] (свежесть последнего блока), `raw.*`.
Секреты: `bsc_dex.wallet_key`, `bsc.node_ws_key`, `bsc.node_http_key`.
