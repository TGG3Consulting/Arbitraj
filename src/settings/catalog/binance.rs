//! Файл: settings/catalog/binance.rs
//! Что делает: настройки, нужные только бирже Binance (решение 160), — пачка потоков в SUBSCRIBE, счёт вывода, пауза
//! истории выводов (вес 18000), минимум депозита из depositDust, остаток монеты скидки (BNB, решение 158), окно «монета
//! ещё не доступна для вывода» после покупки (решение 251). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки — Binance»).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки решения 160. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static BINANCE: &[SettingDef] = &[
    d("Площадки", "venue.ws_streams_per_msg", K::Integer, L::Positive, "потоков", V, "Решение 160, только Binance: сколько потоков стаканов в одном сообщении SUBSCRIBE (в документации предела нет; на соединение — до 1024 потоков и 5 сообщений в секунду, включая pong). Не задана — стаканы Binance не подписываются Kraken (решение 172): сколько символов в одном запросе subscribe канала book (предел в документации не указан — подобрать замером)"),
    d("Площадки", "venue.withdraw_wallet", K::Choice, L::None, "счёт", V, "Решение 160, только Binance: с какого счёта выводить — spot (walletType 0) или funding (walletType 1), POST /sapi/v1/capital/withdraw/apply. Не задана — вывода с Binance нет").choices(&["spot", "funding"]),
    d("Площадки", "venue.withdraw_history_min_ms", K::Integer, L::Positive, "мс", V, "Решение 160, только Binance: не чаще одного запроса истории выводов за столько мс (GET /sapi/v1/capital/withdraw/history весит 18000 при пределе UID 180000 в минуту — ~10 запросов в минуту на аккаунт). Статус вывода ждёт своей очереди. Не задана — вывода с Binance нет (поиск вывода по своему id нужен после отправки — проверяется до неё)"),
    d("Площадки", "venue.withdraw_unready_window_ms", K::Integer, L::Positive, "мс", V, "Решение 251, только Binance: ответ на вывод −4026 («You have insufficient balance») или −4024 («You don't have this asset») не позже столько мс после своей покупки этой монеты — «монета ещё не доступна для вывода»: вывод не создан, повтор — как −4060 по transfer.locked_retry_ms / transfer.locked_retries_max площадки (решение 166). Живые факты 10.11: отказ через 0.35–0.77 с после покупки (CYBER ×11, WLFI). Не задана — отказ сразу, как раньше").optional(),
    d("Площадки", "venue.deposit_dust_as_min", K::Bool, L::None, "да/нет", V, "Решение 160, только Binance: считать поле depositDust справочника сетей (config/getall) минимальным депозитом. Смысл поля в документации не описан — решение Тиграна. Нет или не задано — минимум по venue.deposit_min_absent_ok (да — 0, иначе неизвестен и сделок с переводом на Binance нет)").optional(),
    d("Площадки", "venue.discount_min_balance", K::Decimal, L::Positive, "монет", V, "Решение 158 у Binance: при venue.fee_deduction = bnb комиссия считается со скидкой, только пока свободной монеты скидки (discountAsset, BNB) на споте не меньше этого; меньше, остаток не прочитан или замер устарел — полная ставка в монете сделки. Не задана — скидка в расчёте не применяется").optional(),
    d("Площадки", "venue.discount_balance_poll_s", K::Integer, L::Positive, "с", V, "Решение 158 у Binance: как часто читать остаток монеты скидки (GET /api/v3/account, вес 20). Замер старше двух кругов — устарел, полная ставка. Не задана — остаток не читается, скидка в расчёте не применяется").optional(),
];
