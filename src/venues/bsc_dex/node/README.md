# venues/bsc_dex/node — связь с узлом BSC

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Модуль и chainId BSC mainnet (56, значение протокола со ссылкой) |
| `jsonrpc.rs` | Запрос JSON-RPC 2.0 и разбор ответа/ошибки в единые коды |
| `rpc.rs` | HTTP: одиночный запрос, пачка (batch), общий POST JSON (для каналов отправки) |
| `ws.rs` | Соединение WebSocket: открыть, запрос с ответом, eth_subscribe |
| `ws_task.rs` | Задача соединения: раздача ответов и уведомлений, тишина = обрыв, сырые сообщения на диск, отписка ушедших |
| `heads.rs` | Опрос eth_blockNumber по HTTP раз в `bsc.blocks_poll_ms` (решение 57): последний блок, свежесть (`feed.stale_after_ms`), блок и его время по запросу (eth_getBlockByNumber, раз на блок на всех) |
| `call.rs` | eth_call к контракту с ABI-кодированием (alloy sol!); пачки — через Multicall3 или пачкой JSON-RPC |
| `multicall.rs` | Multicall3.aggregate3 (dex.multicall3): пачка чтений одним eth_call, ответ каждого вызова отдельно |
| `failover.rs` | Резервный провайдер узла: сбой основного (связь, таймаут, 5xx, 429) — запрос на резервный, удержание bsc.node_backup_hold_s |
| `abi.rs` | Описания ERC-20 для ABI |
| `pace.rs` | Темп запросов к узлу HTTP (bsc.node_http_rps; пачка из N — N мест) через очередь queue.rs и правило шлюза на «превышен лимит» (429) |
| `queue.rs` | Очередь мест к узлу со срочностью (gateway/urgency.rs): строгий порядок классов и гарантированная доля фона bsc.node_background_share_pct; отменённый уходит из очереди |
| `source.rs` | Источник событий пулов по bsc.pool_events_source (решение 68): WebSocket или квитанции блоков — одно лицо для слушателей |
| `receipts.rs` | Лента квитанций блоков: подписки logs, eth_blockNumber, eth_unsubscribe, обрыв |
| `receipts_task.rs` | Задача ленты: блоки по порядку без пропусков, отставание, реорг |
| `receipts_io.rs` | Запросы ленты (eth_getBlockByNumber, eth_getBlockReceipts) и раздача логов подпискам |
| `receipts_chain.rs` | Окно разобранных блоков: parentHash, откат с removed: true, раздача по фильтрам |
| `log_filter.rs` | Фильтр logs ({address, topics}) у нас — для квитанций |
