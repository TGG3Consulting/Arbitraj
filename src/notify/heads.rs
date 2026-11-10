//! Файл: notify/heads.rs
//! Что делает: первая строка сообщения Telegram для каждого события каталога (решение 164) — значок уровня и что
//! случилось, по-русски, без внутренних слов и номеров решений. Описание события для экрана «Уведомления» остаётся
//! в notify/catalog.rs (title); здесь — только то, что видит человек в телефоне. Если текст события уже начинается
//! со значка (собран notify/human.rs Msg), заголовок отсюда не добавляется.
//! Кто вызывает: notify/dispatch.rs.
//! От чего зависит: notify/human.rs (значки).
//! Настройки: нет.

use super::human::{INFO, OK, STOP, WARN};

/// Заголовок события. Принимает: код события. Возвращает: «значок заголовок»; нет в списке — ℹ️ и название из каталога.
pub fn head(code: &str, title: &str) -> String {
    match find(code) {
        Some((icon, h)) => format!("{icon} {h}"),
        None => format!("{INFO} {title}"),
    }
}

/// Значок и заголовок события. Возвращает: None — события нет в списке.
#[rustfmt::skip]
fn find(code: &str) -> Option<(&'static str, &'static str)> {
    Some(match code {
        "deal.candidate" => (INFO, "Найдена выгодная сделка"),
        "episode.opened" => (INFO, "Появилась арбитражная ситуация"),
        "episode.closed" => (INFO, "Арбитражная ситуация закончилась"),
        "window.opened" => (INFO, "Запись окна началась"),
        "window.closed" => (INFO, "Запись окна закончена"),
        "window.stopped" => (WARN, "Запись окна остановлена: кончилось место"),
        "deal.buy_filled" => (OK, "Куплено"),
        "deal.transfer_sent" => (INFO, "Перевод отправлен"),
        "deal.transfer_credited" => (OK, "Перевод зачислен"),
        "transfer.to_trading_failed" => (WARN, "Зачислено, но не переведено на торговый счёт"),
        "deal.sell_filled" => (OK, "Продано"),
        "deal.dust" => (INFO, "После продажи остался остаток монеты"),
        "deal.closed" => (OK, "Сделка закрыта"),
        "cycle.dry" => (INFO, "Сухая проверка цикла DEX ↔ DEX"),
        "exec.verified" => (INFO, "Маршрут DEX проверен перед сделкой"),
        "exec.order_not_filled" => (WARN, "Ордер не исполнился"),
        "exec.swap_reverted" => (STOP, "Обмен на DEX откатился"),
        "exec.withdraw_rejected" => (STOP, "Биржа отклонила вывод"),
        "exec.withdraw_fail_pending" => (WARN, "Вывод завис в сети — ждём подтверждения биржи"),
        "exec.transfer_slow" => (WARN, "Перевод идёт дольше нормы"),
        "exec.deposit_not_credited" => (STOP, "Депозит не зачислен"),
        "exec.credit_mismatch" => (WARN, "Монеты пришли, а биржа не подтверждает депозит"),
        "exec.chain_hash_mismatch" => (WARN, "Перевод пришёл, но биржа-отправитель показывает другое"),
        "exec.verify_rejected" => (WARN, "Сделка DEX отменена перед отправкой: цена уехала"),
        "balance.external" => (WARN, "Баланс изменился не ботом"),
        "risk.stop" => (STOP, "Торговля остановлена"),
        "risk.daily_loss" => (STOP, "Торговля остановлена: дневной лимит убытка"),
        "risk.venue_daily_loss" => (STOP, "Площадка на паузе: дневной лимит убытка"),
        "risk.daily_limit_reset" => (INFO, "Лимит дня сброшен"),
        "risk.reconcile_mismatch" => (WARN, "Балансы не сходятся с учётом бота"),
        "risk.reconcile_paused" => (WARN, "Пауза от сверки балансов"),
        "risk.reconcile_resumed" => (OK, "Балансы сошлись — пауза от сверки снята"),
        "risk.auto_resumed" => (OK, "Пауза снята сама — причина ушла"),
        "risk.low_bnb" => (WARN, "Мало BNB на газ"),
        "risk.low_usdt" => (WARN, "Мало USDT для покупок"),
        "venue.disconnected" => (WARN, "Связь с площадкой потеряна"),
        "venue.reconnected" => (OK, "Связь с площадкой восстановлена"),
        "venue.rate_limited" => (WARN, "Слишком частые запросы к площадке"),
        "dex.receipts_stuck" => (WARN, "Данные блоков BSC отстают — пулы перечитаны"),
        "venue.feed_conn_blocked" => (WARN, "Стаканы биржи молчат, но идёт сделка — соединение не трогаю"),
        "venue.feed_conn_giveup" => (STOP, "Соединение стаканов биржи не оживает — нужен человек"),
        "venue.transfer_status" => (INFO, "Изменился ввод или вывод монеты"),
        "venue.delisting" => (WARN, "Монета в делистинге на бирже"),
        "route.quarantine_in" => (WARN, "Направление на паузе (карантин)"),
        "route.quarantine_out" => (OK, "Направление снова торгуется"),
        "coin.check_done" => (INFO, "Проверка монет закончена"),
        "coin.blacklisted" => (WARN, "Монета в чёрном списке"),
        "coin.credit_shortfall" => (WARN, "Пришло меньше монет, чем ожидали — монета в чёрном списке"),
        "coin.sell_slip" => (WARN, "Продажи хуже плана подряд — монета в чёрном списке"),
        "coin.exit_gap" => (WARN, "Реальный выход продажи ниже модели — монета закрыта для входа"),
        "coin.unblacklisted" => (OK, "Монета убрана из чёрного списка"),
        "dex.check_not_configured" => (WARN, "Проверка налогов DEX не настроена"),
        "pool.hook_excluded" => (INFO, "Пул исключён"),
        "stuck.detected" => (WARN, "Монета застряла: продавать сейчас невыгодно"),
        "stuck.loss_alert" => (STOP, "Нужно решение: убыток выше порога"),
        "stuck.relayed" => (INFO, "Монета перегнана на другую площадку"),
        "stuck.market_sold" => (WARN, "Застрявшая монета продаётся по рынку"),
        "manual.record" => (WARN, "Ручное управление: нужно действие"),
        "manual.taken" => (INFO, "Монеты взяты на себя"),
        "manual.returned" => (OK, "Монеты возвращены боту"),
        "balance.sell" => (INFO, "Продажа из панели «Балансы»"),
        "send.channel_rejected" => (WARN, "Канал отправки не принял транзакцию"),
        "send.tx_not_included" => (WARN, "Транзакция не попала в блок"),
        "send.tx_reverted" => (STOP, "Транзакция откатилась — газ потрачен"),
        "send.outcome_unknown" => (STOP, "Итог операции неизвестен — нужна проверка"),
        "outcome.resolved" => (OK, "Исход операции выяснен ботом"),
        "outcome.unresolved" => (STOP, "Исход операции бот не выяснил — нужна проверка"),
        "send.queue_halted" => (STOP, "Кошелёк DEX остановлен до проверки"),
        "dex.swap_waits_inflow" => (WARN, "Обмен на DEX ждёт поступления в кошелёк"),
        "dex.rebuilt_after_approve" => (INFO, "Обмен на DEX пересобран после разрешения"),
        "send.queue_released" => (OK, "Кошелёк DEX снова работает"),
        "executor.deployed" => (INFO, "Развёртывание исполнителя"),
        "bundle.assembled" => (INFO, "Бандл DEX собран"),
        "bundle.simulated" => (OK, "Бандл DEX прошёл проверку"),
        "bundle.sim_reverted" => (WARN, "Бандл DEX не прошёл проверку — не отправлен"),
        "bundle.sent" => (INFO, "Бандл DEX отправлен"),
        "bundle.included" => (OK, "Бандл DEX в блоке"),
        "bundle.not_included" => (WARN, "Бандл DEX не попал в блок — газ не потрачен"),
        "usdt.relayed" => (OK, "USDT перегнаны"),
        "usdt.relay_failed" => (STOP, "Перегон USDT не состоялся"),
        "usdt.not_returned" => (WARN, "USDT не вернулись на площадку покупки"),
        "wallet.added" => (INFO, "Кошелёк DEX добавлен"),
        "wallet.mapping_changed" => (INFO, "Привязка кошельков изменена"),
        "wallet.executor_deployed" => (OK, "Исполнитель кошелька развёрнут"),
        "system.started" => (OK, "Бот запущен"),
        "system.stopped" => (STOP, "Бот остановлен"),
        "system.secrets_unlocked" => (OK, "Секреты разблокированы"),
        "system.settings_changed" => (INFO, "Изменены настройки"),
        _ => return None,
    })
}

/// Есть ли заголовок у каждого события каталога (для теста полноты). Возвращает: коды без заголовка.
pub fn missing() -> Vec<&'static str> {
    super::catalog::EVENTS
        .iter()
        .filter(|e| find(e.code).is_none())
        .map(|e| e.code)
        .collect()
}
