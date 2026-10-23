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
| `GET /api/secrets/status` | Создан ли файл, разблокировано ли, имена секретов (без значений) |
| `POST /api/secrets/create`, `/unlock`, `/lock`, `/password` | Создать файл, разблокировать, заблокировать, сменить пароль |
| `PUT /api/secrets/{name}`, `DELETE /api/secrets/{name}` | Добавить/заменить, удалить секрет (нужен пароль) |
| `GET /api/ws` | Живой поток: `settings`, `journal`, `lagged` |

Ответ ошибки: `{"error": "понятная причина"}`. Чужие Host/Origin отклоняются (`guard.rs`). Авторизации пока нет — запускать только на 127.0.0.1 (открытый вопрос).
