# notify — уведомления в Telegram

| Файл | Что делает |
| --- | --- |
| `catalog.rs` | Список событий по ТЗ и решениям 30–45 (сделки, ошибки, риск, площадки, монеты, застрявшие монеты, ручное управление, отправка транзакций, система, сводка); кнопки событий (`buttons`) |
| `telegram.rs` | Telegram Bot API: getMe, getChat, sendMessage — по https://core.telegram.org/bots/api |
| `tg_text.rs` | Подгонка текста под предел длины Telegram (4096) |
| `tg_buttons.rs` | Bot API для кнопок: sendMessage с InlineKeyboardMarkup, getUpdates (длинный опрос), answerCallbackQuery, editMessageText |
| `callback.rs` | Клавиатура события из каталога; callback_data «код:id» (≤ 64 байт); кнопка → ManualAction |
| `press.rs` | Одно нажатие: только из notify.telegram_chat, команда в канал ядра, ответ на нажатие, правка сообщения |
| `updates.rs` | Приём нажатий: getUpdates длинным опросом, offset, ожидание при сбоях |
| `client.rs` | Собирает клиента из актуальных настроек и секретов |
| `dispatch.rs` | Отправлять ли событие (вкл/выкл, площадки) и отправка (`send`, `send_with_buttons`); итог — в журнал |
| `check.rs` | «Проверить подключение»: токен (getMe) и доступ к чату (getChat) |
| `settings_watch.rs` | Каждое изменение настроек → журнал и событие `system.settings_changed` |

Настройки: `notify.*`, `http.timeout_ms`. Токен — секрет `telegram.bot_token`.
Кнопки (`stuck.loss_alert`: «Продать сейчас», «Беру на себя»): событие отправляется через `Notifier::send_with_buttons`
с id алерта; `updates.rs` слушает getUpdates длинным опросом (`notify.updates_timeout_s`), принимает нажатия только
из `notify.telegram_chat` и отдаёт ядру `ManualCommand` (core/types/control.rs, поле `by` — кто нажал) в канал `CommandTx`.
После ошибки getUpdates — пауза `notify.updates_retry_ms` и повтор (`retry_after` из ответа Telegram сильнее). Канал команд — `core::types::CommandTx` (стык), его даёт `Core::command_channel()`; `wake` подаёт api после разблокировки, смены и блокировки секретов.
Условие события (`notify.event.condition`) проверяется с этапа 5; пока условие задано — событие не отправляется, причина пишется в журнал.
