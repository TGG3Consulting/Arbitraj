# venues — адаптеры площадок

Каждая площадка — изолированная папка: свой REST, свой WebSocket, свои подписки, свой разбор
ответов и ошибок. Наружу — только типы ядра через контракт `gateway/contract.rs`.

**Только по официальной документации площадки** (CLAUDE.md). У каждой функции — ссылка на раздел документации.

Как подключить новую площадку — `docs/ADAPTER_TEMPLATE.md`. Первые адаптеры (`bingx/`, `bsc_dex/`) — этап 2.

| Папка | Площадка |
| --- | --- |
| `gate/` | Биржа Gate (спот, API v4, решение 135) — README.md папки |
`bybit/` — биржа Bybit, API V5, только спот (решение 136): README.md папки — официальные источники и файлы.
| `bingx/` | BingX (спот) |
| `mexc/` | MEXC (спот, API v3, решение 107) |
| `kucoin/` | KuCoin (спот, Classic API, решение 137) |
| `bitget/` | Bitget (спот, Classic API v2, решение 162) |
| `okx/` | OKX (спот, API v5, решение 161) |
| `poloniex/` | Poloniex (спот, Spot API, решение 169) — README.md папки |
| `binance/` | Binance (спот API + Wallet SAPI, решение 160) — README.md папки |
| `xt/` | XT.com (спот, API v4, решение 167) — README.md папки |
| `bsc_dex/` | DEX в сети BSC |
| `kyberswap/` | Исполнение через агрегатор KyberSwap (внутри bsc_dex, решение 91) |
