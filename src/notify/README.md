# notify — уведомления в Telegram

| Файл | Что делает |
| --- | --- |
| `catalog.rs` | Список событий по ТЗ и решениям 30–45 (сделки, ошибки, риск, площадки, монеты, застрявшие монеты, ручное управление, отправка транзакций, система, сводка); кнопки событий (`buttons`) |
| `telegram.rs` | Telegram Bot API: getMe, getChat, sendMessage — по https://core.telegram.org/bots/api |
| `client.rs` | Собирает клиента из актуальных настроек и секретов |
| `dispatch.rs` | Отправлять ли событие (вкл/выкл, площадки) и отправка; итог — в журнал |
| `check.rs` | «Проверить подключение»: токен (getMe) и доступ к чату (getChat) |
| `settings_watch.rs` | Каждое изменение настроек → журнал и событие `system.settings_changed` |

Настройки: `notify.*`, `http.timeout_ms`. Токен — секрет `telegram.bot_token`.
Кнопки (`stuck.loss_alert`: «Продать сейчас», «Беру на себя») — план: `updates.rs` слушает getUpdates длинным опросом
(`notify.updates_timeout_s`), принимает нажатия только из `notify.telegram_chat` и отдаёт ядру `ManualCommand` (core/types/control.rs).
Условие события (`notify.event.condition`) проверяется с этапа 5; пока условие задано — событие не отправляется, причина пишется в журнал.
