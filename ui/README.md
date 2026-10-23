# ui — веб-интерфейс ARB (Svelte 5 + Vite)

Одностраничное приложение на русском. Всё, что видно и меняется, — через REST и WebSocket бэкенда (`src/api/`).
Значений по умолчанию в интерфейсе нет: не задано — показывается «не задано».
Значения секретов не показываются и не хранятся в браузере (поля `type=password`, после отправки очищаются).

## Сборка и запуск

```sh
cd ui && npm ci && npm run build          # → ui/dist
cargo run -- --data-dir <папка> --listen 127.0.0.1:8080 --live-buffer 1024 --ui-dir ui/dist
# открыть http://127.0.0.1:8080
```

Без `--ui-dir` бэкенд отдаёт только API. Неизвестный адрес под `/api/` — 404 с причиной;
любой другой адрес — `index.html` (экраны открываются по прямой ссылке `#/экран`).

## Разработка (сервер Vite с proxy на бэкенд)

```sh
# 1. Бэкенд: разрешить страницы с адреса сервера Vite (защита Origin, src/api/guard.rs)
cargo run -- --data-dir <папка> --listen 127.0.0.1:8080 --live-buffer 1024 --allow-origin http://localhost:5173
# 2. Интерфейс: адрес бэкенда — переменная ARB_API (значения по умолчанию нет)
cd ui && ARB_API=http://127.0.0.1:8080 npm run dev
```

Proxy подменяет Host на адрес бэкенда (`changeOrigin`), Origin браузера передаёт как есть — поэтому нужен `--allow-origin`
с тем адресом, который напечатал Vite. WebSocket `/api/ws` идёт через тот же proxy.

## Экраны и откуда данные

| Экран (`#/…`) | Файл | Данные и действия |
| --- | --- | --- |
| Разблокировка (`unlock`) | `src/pages/Unlock.svelte` | `GET /api/secrets/status`, `/known`; `POST /api/secrets/create`, `/unlock`, `/lock`, `/password`; `PUT`/`DELETE /api/secrets/{name}` |
| Площадки (`venues`) | `src/pages/Venues.svelte` | `GET /api/venues`; настройки области Venue из схемы; секреты площадки (`scope` = id); `POST /api/check/venue/{id}`; у DEX — `GET /api/venues/{id}/dex-families` (область «DEX»), `GET /api/channels/{id}` (ключ канала, `POST /api/check/channel/{venue}/{id}`) |
| Настройки (`settings`) | `src/pages/Settings.svelte` | `GET /api/settings/schema`, `GET`/`PUT /api/settings`, `GET /api/settings/history`; области «DEX» и «канал» — id от адаптера DEX (нет адаптера — «нет данных»); виды значений: выбор из списка, адрес BSC, верхняя граница целого |
| Монеты (`coins`) | `src/pages/Coins.svelte` | `GET /api/coins` (503 — «нет данных»), `POST /api/coins/recheck`; ручной чёрный список — `PUT /api/settings` (`coin.blocked`, `coin.blocked_note`); WS `coin_check`, `pool_check`, `coin_block` |
| Ручное управление (`manual`) | `src/pages/Manual.svelte` | `GET /api/manual`, `POST /api/manual/command`; WS `stuck_alert`, `manual` |
| Каналы отправки (`channels`) | `src/pages/Channels.svelte` | `GET /api/channels/{venue}`, настройки области «канал», ключ канала, `POST /api/check/channel/{venue}/{id}`; WS `send_channel` |
| Статистика газа (`gas`) | `src/pages/GasStats.svelte` | `GET /api/gas-stats`; WS `gas_stats` |
| Правила шлюза (`rules`) | `src/pages/Rules.svelte` | `GET`/`POST /api/rules`, `PUT /api/rules/{id}`, `GET /api/rules/meta`, `GET /api/venues` |
| Уведомления (`notify`) | `src/pages/Notifications.svelte` | события из схемы; `notify.*`, `http.timeout_ms`; секрет `telegram.bot_token`; `POST /api/check/telegram` |
| Журнал (`journal`) | `src/pages/Journal.svelte` | `GET /api/journal?limit=N&before=ID`; живые записи из `/api/ws` |

На всех экранах: плашка «Система заблокирована», пока секреты не разблокированы; плашка «Нет живой связи»,
пока WebSocket не подключён. Изменения из других вкладок приходят через `/api/ws`
(`settings` — значения, `journal` — записи; по записям `secrets` и `rules.*` перечитываются секреты и правила;
`lagged` и переподключение — перечитать всё по REST).

## Папки

| Путь | Что внутри |
| --- | --- |
| `index.html`, `vite.config.js`, `package.json` | Страница, сборка и сервер разработки, зависимости (точные версии) |
| `src/` | Код интерфейса (`src/README.md`) |
| `dist/` | Результат сборки (не в git) |
