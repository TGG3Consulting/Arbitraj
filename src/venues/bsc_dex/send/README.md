# venues/bsc_dex/send — каналы отправки транзакций (решения 33, 41–45)

Каждый канал — свой файл по своей официальной документации (ссылки — в шапке). Общий вид — `channel.rs`.
Канал с ключом без ключа не используется. Открытый канал — только один и не вместе с закрытыми; несколько
закрытых — одна и та же транзакция параллельно, без чаевых. Чаевые — только при одном канале и бандле.
Попадание в блок — только своим узлом (`track.rs`).

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Выбор готовых каналов и правила, нижняя граница цены газа, параллельная отправка, ожидаемые чаевые |
| `channel.rs` | Вид канала: одиночная/бандл и их форматы, закрытый ли, чаевые, ключ (адрес или заголовок) |
| `channels.rs` | Список каналов адаптера (id — ARCHITECTURE.md) |
| `ready.rs` | Готовность по настройкам `channel.*` и ключу; тело запроса в формате канала; разбор ответа |
| `check.rs` | «Проверить подключение» канала: пустая транзакция — канал должен ответить ошибкой JSON-RPC |
| `track.rs` | Квитанция на каждый новый блок: в блоке / откат / не попала / вытеснена |
| `stats.rs` | Счётчики каналов и событие SendChannel |
| `public.rs` | Открытая отправка (свой узел) |
| `mev_guard.rs` | PancakeSwap MEV Guard |
| `club48.rs` | 48 Club (Privacy RPC / Puissant) |
| `blockrazor.rs` | BlockRazor RPC (только одиночная: документация бандла противоречива) |
| `blockrazor_builder.rs` | BlockRazor Builder (ключ) |
| `blink.rs` | Blink: bsc.merkle.io, bsc.blink.io, личный с ключом — три канала |
| `jetbuilder.rs` | JetBuilder |
| `nodereal.rs` | NodeReal Builder |
| `nodereal_api.rs` | NodeReal Bundle Service API (ключ, только бандл) |
| `bloxroute_protect.rs` | bloXroute Protect RPC (бесплатный) |
| `bloxroute_cloud.rs` | bloXroute Cloud API (ключ) |
| `flashblock.rs` | Flashblock (ключ, только бандл) |
