# Адаптер KyberSwap (агрегатор) — площадка DEX через API, без чтения пулов

> Назначение: торговать DEX-сторону через **агрегатор KyberSwap** вместо собственного движка пулов/графа
> (`bsc_dex`). Цена берётся из REST-API агрегатора (Kyber сам обходит все DEX и отдаёт лучший маршрут) —
> узел для цен НЕ нужен. Узел нужен только чтобы отправить один своп на сделку. Второй адаптер DEX, по тому же
> контракту `gateway/contract.rs`; ядро, шлюз и `bsc_dex` не меняются. Выбирается вместо `bsc_dex` при запуске.
>
> ПРАВИЛО ПРОЕКТА: всё строго по официальной документации Kyber, ничего не додумывать. Чего нет в документации —
> так и помечено ниже. Ссылки на разделы — у каждой функции адаптера в комментарии.

## Официальная документация
- Спецификация API агрегатора (EVM-свопы): https://docs.kyberswap.com/kyberswap-solutions/kyberswap-aggregator/aggregator-api-specification/evm-swaps
- Обзор API агрегатора: https://docs.kyberswap.com/Aggregator/aggregator-api

## Сеть
- BSC поддерживается: сегмент пути сети — имя `bsc` (ChainID 56). Сеть в пути, не chainId.

## Доступ (аутентификация и лимиты)
- Прямые эндпоинты: заголовок `X-Client-Id` (свой идентификатор; без него — строже лимиты). Для старта — свой id,
  без регистрации и платного ключа.
- Шлюз `https://api.kyberswap.com/swap/` (выше лимиты): заголовок `X-Api-Key` (ключ — через Business Development).
- Конкретные числа лимитов в документации не указаны («не указано в документации»). На практике (со слов, не офдок)
  агрегатор лимитами не упирается.

## Поток (две REST-операции + одна транзакция)

### 1) Квота (цена) — GET
`/{chain}/api/v1/routes`
- Заголовок: `X-Client-Id`.
- Обязательные query: `tokenIn`, `tokenOut`, `amountIn` (в wei).
- Необязательные: `includedSources`, `excludedSources`, `excludeRFQSources`, `gasInclude`, `gasPrice`,
  `feeAmount`, `chargeFeeBy`, `isInBps`, `feeReceiver`, `origin`.
- Ответ: `data.routeSummary` { `amountOut`, `gas`, `gasPrice`, `gasUsd`, `route`, `routeID`, `checksum`,
  `timestamp` } и `data.routerAddress`.
- Маршрут не кэшировать дольше 5–10 секунд (рекомендация документации).

### 2) Сборка транзакции — POST
`/{chain}/api/v1/route/build`
- Тело: `routeSummary` (ровно как из GET routes), `sender`, `recipient`. Необязательные: `slippageTolerance`
  (bps, 0–2000, по умолчанию 0), `deadline` (Unix-секунды, по умолчанию +20 минут).
- Ответ `data`: `data` (готовый calldata), `routerAddress`, `transactionValue` (для свопов с нативной монетой),
  `amountIn`, `amountOut`, `gas` и поля в USD.

### 3) Отправка свопа
- `approve` токена входа на `routerAddress` (адрес берётся из ответа, не зашит), затем транзакция на `routerAddress`
  с `data` = calldata и `value` = `transactionValue`. Отправка/подпись/газ — наш кошелёк и узел (как в `bsc_dex`).

## Как ложится на контракт адаптера (gateway/contract.rs)
- `kind()` → `Dex`.
- `quote(asset, side, qty)` → GET routes (USDT↔монета), `amountOut` → цена; исполнимый объём = запрошенный
  (агрегатор сам дробит по DEX). Комиссии/газ — из ответа (`gas`, `gasUsd`).
- `balances()` → баланс кошелька (BNB + токены), как в `bsc_dex/wallet/balances.rs` (можно переиспользовать).
- `trade(req)` → POST build → отправка свопа через роутер Kyber.
- `withdraw`/`deposit`/`withdraw_cost` → переводы кошелька (как у DEX-стороны BingX↔DEX).
- `subscribe_market` → поллинг квот (у агрегатора нет потока цен): периодически запрашивать маршруты по монетам и
  отдавать ядру цены. Частота — настройкой.
- Пуловые методы (`pool_directory`, `pool_depths`, `dex_families`, `dex_cycles`, `send_channels`) — по умолчанию
  (пустые): у агрегатора своих пулов/каналов нет.

## Настройки (каталог, без хардкодов) — предварительно
- `kyber.base_url` — база API (прямые эндпоинты или шлюз).
- `kyber.chain` — имя сети (`bsc`).
- `kyber.client_id` — заголовок X-Client-Id (секрет/настройка).
- `kyber.quote_poll_ms` — частота поллинга квот для живого экрана.
- `kyber.slippage_bps` — допуск проскальзывания при сборке транзакции.
- (точный список — при реализации, с зеркалом в docs/SETTINGS.md).

## Статус
- Фаза 1 (в работе): квота/сигнал (сухой режим) — цены из агрегатора на живой экран, сверка с BingX, без исполнения.
- Фаза 2: исполнение (approve + build + отправка свопа), переводы, пять частей фичи, UI-переключатель, решение в DECISIONS.
