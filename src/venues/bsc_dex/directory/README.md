# venues/bsc_dex/directory — справочник пулов от индексатора (решение 58)

Список пулов монеты берётся у индексатора по REST (как справочник монет BingX), а не перебором фабрик.
Индексатор — **только справочник адресов**: цены и состояние пулов — со своего узла (подписки logs + чтение состояния).
Основной источник — GeckoTerminal / CoinGecko on-chain; DexScreener — второй источник позже.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://docs.coingecko.com/demo/reference/top-pools-contract-address | `GET /onchain/networks/{network}/tokens/{token_address}/pools`: `page`, `include=base_token,quote_token,dex`, «up to 20 pools per page», >10 страниц — тариф Analyst, ключ `x-cg-demo-api-key`, поля пула |
| https://docs.coingecko.com/demo/reference/endpoint-overview | `GET /onchain/networks/{network}/dexes` и прочие эндпоинты Demo |
| https://docs.coingecko.com/docs/keyless-public-api | Без ключа: `https://api.geckoterminal.com/api/v2`, ≈10 запросов в минуту; заголовки ключей Demo `x-cg-demo-api-key`, Pro `x-cg-pro-api-key` |
| https://docs.coingecko.com/docs/errors-and-rate-limits | Лимиты в минуту, ошибки тоже считаются, превышение — 429 |
| https://brand.coingecko.com/resources/attribution-guide | Подпись: «On-chain data provided by GeckoTerminal» → https://www.geckoterminal.com |
| tests/data/indexers_2026-10-24 | Живые ответы: формат пулов и DEX, PoolId у V4/Infinity, 400 на странице за пределами |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Состояние справочника (очередь, DEX индексатора, пулы по монетам и из событий, итог по пулу, взятые и исключённые пулы), отбор пар с USDT/промежуточными |
| `gt_parse.rs` | Разбор ответов: страница пулов (data + included), страница DEX (links.next), текст ошибки |
| `gt_http.rs` | GET к индексатору: адрес, ключ в заголовке, очередь, сырой журнал, коды ответа; 400 на странице > 1 — «страниц больше нет» |
| `limit.rs` | Темп: 60 / index.requests_per_min секунд на запрос; 429 — правило шлюза RateLimited (как у узла) |
| `fetch.rs` | Запросы: пулы монеты по страницам, список DEX сети по links.next |
| `refresh.rs` | Обновление: монеты, у которых подошёл срок index.refresh_s, по страницам; список DEX; ошибки — в интерфейс и журнал (dex.index) |
| `store.rs` | Ответы в SQLite (venue_state): после перезапуска свежие монеты не спрашиваются |
| `view.rs` | Справочник для интерфейса (pool_directory): пулы с итогом, нативный BNB — «не поддержан», пулы из событий создания |
| `check.rs` | Часть «Проверить подключение» DEX: первая страница списка DEX сети |

## Расход запросов

Монета — `1…index.pages_per_token` запросов раз в `index.refresh_s` (страница короче `index.page_size` — последняя);
список DEX сети — 1–2 запроса за тот же срок. Demo: 10 000 в месяц — например 200 монет × 2 страницы раз в сутки ≈ 12 000,
значит интервал около 1,5 суток или Basic.
