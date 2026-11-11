# venues — адаптеры площадок

Каждая площадка — изолированная папка: свой REST, свой WebSocket, свои подписки, свой разбор
ответов и ошибок. Наружу — только типы ядра через контракт `gateway/contract.rs`.

**Только по официальной документации площадки** (CLAUDE.md). У каждой функции — ссылка на раздел документации.

Как подключить новую площадку — `docs/ADAPTER_TEMPLATE.md`.

Решение 217: каждая биржа отдаёт снимок стакана (`book.rs` `freeze` → `gateway/frozen.rs` `FrozenBook`, контракт
`quote_snapshot`); её живая котировка — тот же снимок на один вызов. DEX — `bsc_dex/frozen.rs`. KyberSwap снимка не
отдаёт (не площадка цен — `NotApplicable` по умолчанию). Первые адаптеры (`bingx/`, `bsc_dex/`) — этап 2.

Решение 254: если справочник пар биржи перечисляет допустимые типы ордера / timeInForce, адаптер при разборе
справочника (подписка и фоновое обновление) ставит паре `SpotPairState::order_block` — причину «биржа не примет наш
ордер» той же функцией, что проверяет ордер (`order_rules::order_block`); ядро такую пару не считает ни для покупки,
ни для продажи (`core/presence.rs` OrderTypeNotAccepted, `core/pairs_fresh.rs`). Поля есть у XT (orderTypes,
timeInForces), Binance (orderTypes), MEXC (orderTypes). У BingX, Bybit, Gate, KuCoin, OKX, Bitget, HTX, CoinEx,
Phemex, Poloniex, Kraken в справочнике пар таких полей нет — признак не ставится (режимы «только post-only» и т. п. у
OKX, Poloniex, Kraken — это статус пары: такие пары и так не торгуются, PairNotTrading).

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
| `phemex/` | Phemex (спот, решение 168) — README.md папки |
| `binance/` | Binance (спот API + Wallet SAPI, решение 160) — README.md папки |
| `xt/` | XT.com (спот, API v4, решение 167) — README.md папки |
| `kraken/` | Kraken (спот REST + Funding (Beta) + WebSocket v2, решение 172) — README.md папки |
| `coinex/` | CoinEx (спот, API v2, решение 170) — README.md папки |
| `htx/` | HTX, бывший Huobi (спот API v1/v2, решение 171) — README.md папки |
| `bsc_dex/` | DEX в сети BSC |
| `kyberswap/` | Исполнение через агрегатор KyberSwap (внутри bsc_dex, решение 91) |
