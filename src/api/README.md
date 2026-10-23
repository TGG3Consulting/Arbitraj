# api — веб-сервер для интерфейса (axum)

| Маршрут | Что делает |
| --- | --- |
| `GET /api/settings/schema` | Каталог настроек и событий уведомлений |
| `GET /api/settings` | Все заданные значения |
| `PUT /api/settings` | `{name, scope, value}`; `value: null` — очистить |
| `GET /api/settings/history?limit=N[&name=]` | История изменений |
| `GET /api/rules`, `POST /api/rules`, `PUT /api/rules/{id}` | Правила шлюза |
| `GET /api/journal?limit=N[&before=ID]` | Журнал |
| `POST /api/check/telegram` | «Проверить подключение» Telegram |
| `POST /api/check/venue/{id}` | «Проверить подключение» площадки |
| `GET /api/ws` | Живой поток: `settings`, `journal`, `lagged` |

Ответ ошибки: `{"error": "понятная причина"}`. Авторизации пока нет — запускать только на 127.0.0.1 (открытый вопрос).
