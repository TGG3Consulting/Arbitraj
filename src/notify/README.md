# notify — уведомления в Telegram

| Файл | Что делает |
| --- | --- |
| `catalog.rs` | Список событий по ТЗ (группы: сделки, ошибки, риск, площадки, монеты, система, сводка) |
| `telegram.rs` | Telegram Bot API: getMe, getChat, sendMessage — по https://core.telegram.org/bots/api |
| `client.rs` | Собирает клиента из актуальных настроек и секретов |
| `dispatch.rs` | Отправлять ли событие (вкл/выкл, площадки) и отправка; итог — в журнал |
| `check.rs` | «Проверить подключение»: токен (getMe) и доступ к чату (getChat) |
| `settings_watch.rs` | Каждое изменение настроек → журнал и событие `system.settings_changed` |

Настройки: `notify.*`, `http.timeout_ms`. Токен — секрет `telegram.bot_token`.
Условие события (`notify.event.condition`) проверяется с этапа 5; пока условие задано — событие не отправляется, причина пишется в журнал.
