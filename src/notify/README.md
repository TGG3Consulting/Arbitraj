# notify — уведомления в Telegram

| Файл | Что делает |
| --- | --- |
| `catalog.rs` | Список событий по ТЗ и решениям 30–45 (сделки, ошибки, риск, площадки, монеты, застрявшие монеты, ручное управление, отправка транзакций, система); кнопки событий (`buttons`) |
| `telegram.rs` | Telegram Bot API: getMe, getChat, sendMessage — по https://core.telegram.org/bots/api |
| `tg_text.rs` | Подгонка текста под предел длины Telegram (4096) |
| `tg_buttons.rs` | Bot API для кнопок: sendMessage с InlineKeyboardMarkup, getUpdates (длинный опрос), answerCallbackQuery, editMessageText |
| `callback.rs` | Клавиатура события из каталога; callback_data «код:id» (≤ 64 байт) |
| `press.rs` | Одно нажатие: только из notify.telegram_chat, разбор callback_data и передача обработчику |
| `press_record.rs` | Кнопка записи «Ручного управления»: по текущим записям экрана; нет записи — «уже выполнено»; недоступна — причина; ввод суммы — ForceReply; иначе подтверждение |
| `press_confirm.rs` | «Да, …» / «Отмена»: команда ядру (Core::command, источник Telegram, кто нажал), правка вопроса «Выполнено: кто, что, итог» |
| `mirror.rs`, `mirror_send.rs` | Дубль экрана «Ручное управление» (решение 156): сообщение на новую запись, напоминание пачкой каждые `notify.manual_repeat_s`, правка ушедших записей, 429 — пауза retry_after |
| `mirror_state.rs` | Память дубля (отправленные, ушедшие, выполненные, ожидания «Да», вопросы суммы) — чистые функции, тесты tests/notify_mirror.rs |
| `mirror_view.rs` | Текст и кнопки записи и напоминания — из записей экрана (core/manual_feed.rs) |
| `menu.rs`, `menu_press.rs` | Меню /menu (/dev): Состояние, Ручное управление, Режим системы, Площадки и карантин, Лимит дня — как плашка режима; навигация editMessageText, «Назад» (решение 157) |
| `menu_day.rs` | Экран «Лимит дня» и строки лимита для «Состояния»: общий лимит, итог и лимит дня по площадкам, кнопки «Сбросить лимит дня (все)» (c:dr.x) и по площадке (c:dv.<площадка>) — решение 173 |
| `tg_commands.rs` | Сообщения: /help, /menu, /dev, /status, /manual, /start, /resume, /pause, /stop; ответ на вопрос суммы |
| `tg_methods.rs` | Bot API: sendMessage с разметкой (message_id), editMessageText с кнопками, editMessageReplyMarkup, setMyCommands, ForceReply |
| `updates_idle.rs` | Ожидание приёма после сбоев (retry_after, notify.updates_retry_ms, wake) |
| `updates.rs` | Приём нажатий и сообщений: getUpdates (message, callback_query) длинным опросом, offset; после подключения getMe и setMyCommands |
| `client.rs` | Собирает клиента из актуальных настроек и секретов |
| `dispatch.rs` | Отправлять ли событие (вкл/выкл, площадки) и отправка (`send`, `send_with_buttons`); итог — в журнал |
| `heads.rs` | Первая строка Telegram каждого события: значок уровня и что случилось по-русски (решение 164) |
| `human.rs` | Человеческий вид алертов: деньги и проценты — 2 знака, количество монеты, имена площадок, короткий адрес, сборщик `Msg` (заголовок, факты, «Причина:», «Что делать:», id) |
| `human_msg.rs` | Сборщик алерта `Msg` и разбор текста ядра «<id сделки>: …» (`split_deal`, `from_core`) |
| `reason.rs` | Человеческая причина из текста ошибки: известный код биржи — формулировка, неизвестный — «Ответ биржи: …» (обрезано) |
| `reason_codes.rs` | Словарь кодов ошибок BingX, MEXC, Gate, Bybit, KuCoin → человеческие формулировки (смысл — из venues/*/errors.rs) |
| `check.rs` | «Проверить подключение»: токен (getMe) и доступ к чату (getChat) |
| `settings_watch.rs` | Каждое изменение настроек → журнал и событие `system.settings_changed` |

Настройки: `notify.*` (в т.ч. `notify.manual_repeat_s` — напоминание «Ручного управления»), `http.timeout_ms`. Токен — секрет `telegram.bot_token`.
Решение 156: записи экрана «Ручное управление» (core/manual_feed.rs) уходят событием `manual.record` с теми же кнопками,
что на экране; нажатие → подтверждение «Да, …» → та же команда ядра, что у экрана (`Core::command`, источник — Telegram,
`by` — кто нажал). Напоминание — одно сообщение на все ждущие записи (лимиты Telegram), 429 — пауза `retry_after`.
Решение 157: команды бота (`tg_commands.rs`), меню (`menu*.rs`) — те же команды ядра, что плашка режима; настроек из Telegram нет.
`updates.rs` принимает `message` и `callback_query` только из `notify.telegram_chat`. После ошибки getUpdates — пауза
`notify.updates_retry_ms` (`retry_after` сильнее); `wake` подаёт api после разблокировки, смены и блокировки секретов.
Условие события (`notify.event.condition`) проверяется с этапа 5; пока условие задано — событие не отправляется, причина пишется в журнал.
