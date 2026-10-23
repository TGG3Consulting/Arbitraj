//! Файл: settings/catalog.rs
//! Что делает: полный список настроек системы — зеркало docs/SETTINGS.md. Значений нет.
//! Кто вызывает: settings/store.rs (проверка имени и типа), api/settings.rs (схема для интерфейса).
//! От чего зависит: settings/schema.rs.
//! Настройки: это и есть их список.
//!
//! Новая настройка: строка в docs/SETTINGS.md + строка здесь + поле в интерфейсе.

use super::schema::{Kind as K, Limit as L, ScopeKind as S, SettingDef};

/// Короткая запись одной настройки. Принимает: группу, имя, тип, ограничение, единицу, области, описание.
/// Возвращает: SettingDef.
const fn d(
    group: &'static str,
    name: &'static str,
    kind: K,
    limit: L,
    unit: &'static str,
    scopes: &'static [S],
    description: &'static str,
) -> SettingDef {
    SettingDef {
        name,
        kind,
        limit,
        unit,
        scopes,
        description,
        group,
    }
}

const G: &[S] = &[S::Global];
const GV: &[S] = &[S::Global, S::Venue];
const GC: &[S] = &[S::Global, S::Coin];
const V: &[S] = &[S::Venue];
const VC: &[S] = &[S::Venue, S::Coin];
const E: &[S] = &[S::Event];

/// Все настройки системы. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static CATALOG: &[SettingDef] = &[
    d("Деньги и риск", "money.min_profit_usd", K::Decimal, L::Positive, "USDT", G, "Минимальная прибыль сделки после всех затрат"),
    d("Деньги и риск", "money.min_profit_pct", K::Decimal, L::Percent, "%", G, "Минимальная прибыль в процентах от вложенного"),
    d("Деньги и риск", "money.max_per_trade_usd", K::Decimal, L::Positive, "USDT", GV, "Максимум вложений в одну сделку"),
    d("Деньги и риск", "money.max_per_coin_usd", K::Decimal, L::Positive, "USDT", G, "Максимум вложений в одну монету одновременно"),
    d("Деньги и риск", "money.max_in_transit_usd", K::Decimal, L::Positive, "USDT", G, "Максимум суммы во всех переводах одновременно"),
    d("Деньги и риск", "risk.max_daily_loss_usd", K::Decimal, L::Positive, "USDT", G, "Убыток за день, после которого система останавливается"),
    d("Деньги и риск", "risk.min_bnb_balance", K::Decimal, L::NonNegative, "BNB", G, "Минимальный остаток BNB на кошельке для газа"),
    d("Расчёт выгодности", "profit.price_move_k", K::Decimal, L::NonNegative, "число", GC, "Коэффициент осторожности k в запасе r = k · σ · √T"),
    d("Расчёт выгодности", "profit.liquidity_share_h", K::Decimal, L::Fraction, "доля 0–1", V, "Доля глубины стакана или пула B, которую разрешено использовать"),
    d("Расчёт выгодности", "profit.default_transit_time_s", K::Integer, L::Positive, "секунды", V, "Время в пути, пока нет истории переводов по монете"),
    d("Сигнал", "signal.min_hold_ms", K::Integer, L::NonNegative, "мс", G, "Сколько спред должен держаться, чтобы считаться сигналом"),
    d("Сигнал", "signal.min_hold_updates", K::Integer, L::Positive, "штук", G, "Сколько обновлений подряд спред должен держаться"),
    d("Сигнал", "signal.anomaly_spread_pct", K::Decimal, L::Positive, "%", G, "Спред выше этого — аномалия, нужна перепроверка статусов"),
    d("Сигнал", "signal.route_cooldown_s", K::Integer, L::NonNegative, "секунды", G, "Пауза направления после сделки или ошибки"),
    d("Сигнал", "feed.stale_after_ms", K::Integer, L::Positive, "мс", V, "Цена без обновлений дольше этого считается устаревшей"),
    d("Исполнение", "exec.dex_slippage_pct", K::Decimal, L::Percent, "%", V, "Допуск проскальзывания в свопе на DEX (минимум на выходе)"),
    d("Исполнение", "exec.sell_wait_max_s", K::Integer, L::NonNegative, "секунды", G, "Сколько ждать лучшей цены, если спред схлопнулся за время перевода"),
    d("Исполнение", "exec.max_loss_on_sell_pct", K::Decimal, L::Percent, "%", G, "Допустимый убыток при вынужденной продаже"),
    d("Переводы и контроль", "transfer.max_time_s", K::Integer, L::Positive, "секунды", VC, "Перевод дольше этого — карантин и уведомление"),
    d("Переводы и контроль", "ledger.reconcile_interval_s", K::Integer, L::Positive, "секунды", G, "Как часто сверять балансы"),
    d("Переводы и контроль", "risk.max_errors_in_row", K::Integer, L::Positive, "штук", V, "Ошибок API или вывода подряд до паузы площадки"),
    d("Площадки", "venue.enabled", K::Bool, L::None, "да/нет", V, "Площадка включена"),
    d("Площадки", "venue.rest_url", K::Url, L::None, "URL", V, "Адрес REST API площадки (из её официальной документации)"),
    d("Площадки", "venue.ws_url", K::Url, L::None, "URL", V, "Адрес WebSocket площадки (из её официальной документации)"),
    d("Площадки", "venue.api_ip_bound", K::Bool, L::None, "да/нет", V, "Ключ API привязан к IP: да — нужен IP, нет — привязка не обязательна"),
    d("Площадки", "venue.api_ip", K::Ip, L::None, "IP", V, "IP, к которому привязан ключ API (нужен, если привязка включена)"),
    d("Площадки", "venue.ws_symbols_per_conn", K::Integer, L::Positive, "штук", V, "Сколько монет в одном WebSocket-соединении (в пределах лимита площадки)"),
    d("Площадки", "venue.status_poll_ms", K::Integer, L::Positive, "мс", V, "Частота опроса статусов ввода и вывода, если у площадки нет WebSocket для них"),
    d("Площадки", "venue.rest_fallback_url", K::Url, L::None, "URL", V, "Резервный адрес REST — только при сбое сети основного (из официальной документации площадки)"),
    d("Площадки", "venue.recv_window_ms", K::Integer, L::Positive, "мс", V, "Окно действия подписанного запроса (у BingX не больше 5000)"),
    d("Площадки", "venue.ws_subs_per_sec", K::Integer, L::Positive, "в секунду", V, "Сколько подписок WebSocket отправлять в секунду (у BingX не больше 10)"),
    d("Площадки", "venue.ws_silence_ms", K::Integer, L::Positive, "мс", V, "Нет сообщений дольше — соединение считается оборванным и переподключается"),
    d("Площадки", "venue.ws_reconnect_pause_ms", K::Integer, L::NonNegative, "мс", V, "Пауза перед переподключением WebSocket"),
    d("Площадки", "venue.depth_levels", K::Integer, L::Positive, "уровней", V, "Сколько уровней стакана получать (у BingX: 5, 10, 20, 50, 100)"),
    d("Площадки", "venue.order_time_in_force", K::Text, L::None, "тип", V, "Срок действия ордера (у BingX: IOC, FOK, GTC, PostOnly)"),
    d("Площадки", "venue.withdraw_wallet_type", K::Integer, L::Positive, "код", V, "С какого счёта выводить (у BingX: 1 — фондовый, 4 — спот)"),
    d("Площадки", "venue.bsc_network", K::Text, L::None, "название", V, "Как площадка называет сеть BSC (у BingX: BEP20)"),
    d("Площадки", "venue.quote_asset", K::Text, L::None, "монета", V, "Монета, за которую покупаем и продаём (USDT)"),
    d("Площадки", "venue.listen_key_renew_s", K::Integer, L::Positive, "секунды", V, "Как часто продлевать ключ приватного потока (у BingX живёт 1 час)"),
    d("Площадки", "venue.fee_cache_s", K::Integer, L::Positive, "секунды", V, "Сколько держать ставку торговой комиссии без повторного запроса"),
    d("Площадки", "bsc.node_ws_url", K::Url, L::None, "URL", G, "Адрес узла BSC (провайдер или свой узел)"),
    d("Площадки", "dex.enabled", K::Bool, L::None, "да/нет", V, "Конкретная DEX включена"),
    d("Площадки", "dex.min_pool_liquidity_usd", K::Decimal, L::NonNegative, "USDT", V, "Пулы с ликвидностью ниже не слушаются"),
    d("Уведомления", "notify.telegram_api_url", K::Url, L::None, "URL", G, "Адрес Telegram Bot API (из официальной документации core.telegram.org/bots/api)"),
    d("Уведомления", "notify.telegram_chat", K::Text, L::None, "ID чата", G, "Куда слать уведомления"),
    d("Уведомления", "notify.event.enabled", K::Bool, L::None, "да/нет", E, "Присылать ли это событие"),
    d("Уведомления", "notify.event.venues", K::TextList, L::None, "список", E, "Для каких площадок присылать (пусто — для всех)"),
    d("Уведомления", "notify.event.condition", K::Text, L::None, "зависит от события", E, "Условие отправки, где оно имеет смысл (формат — этап 5)"),
    d("Уведомления", "notify.summary_interval_s", K::Integer, L::Positive, "секунды", G, "Как часто присылать сводку"),
    d("Безопасность", "security.kdf_memory_kib", K::Integer, L::Positive, "КиБ", G, "Argon2id: память на вывод ключа из пароля (OWASP: не меньше 19456)"),
    d("Безопасность", "security.kdf_iterations", K::Integer, L::Positive, "проходов", G, "Argon2id: число проходов (OWASP: не меньше 2 при 19456 КиБ)"),
    d("Безопасность", "security.kdf_parallelism", K::Integer, L::Positive, "потоков", G, "Argon2id: число потоков"),
    d("Безопасность", "security.password_min_len", K::Integer, L::Positive, "символов", G, "Минимальная длина пароля секретов"),
    d("Система", "raw.enabled", K::Bool, L::None, "да/нет", G, "Писать сырые сообщения площадок на диск (zstd)"),
    d("Система", "raw.zstd_level", K::Integer, L::Positive, "уровень", G, "Уровень сжатия zstd для сырых сообщений"),
    d("Система", "raw.rotate_s", K::Integer, L::Positive, "секунды", G, "Как часто начинать новый файл сырых сообщений"),
    d("Система", "http.timeout_ms", K::Integer, L::Positive, "мс", G, "Сколько ждать ответа внешнего сервиса по HTTP (Telegram, REST площадок)"),
];

/// Ищет описание настройки по имени.
/// Принимает: имя. Возвращает: описание или None, если такой настройки нет.
pub fn find(name: &str) -> Option<&'static SettingDef> {
    CATALOG.iter().find(|s| s.name == name)
}
