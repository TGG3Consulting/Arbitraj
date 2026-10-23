//! Файл: settings/catalog/trade.rs
//! Что делает: настройки торговой политики ядра и проверки монет — налоги токена, крючки V4,
//! возврат USDT, застрявшие монеты, ручной чёрный список. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, решения 30, 32, 34, 36, 37).

use super::{C, G, GV, V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Максимум ожидания застрявшей монеты — 3 часа в мс (решение 37: «ждать до N мс, максимум 3 часа»).
const STUCK_WAIT_MAX_MS: u64 = 3 * 60 * 60 * 1000;

/// Настройки торговой политики. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static TRADE: &[SettingDef] = &[
    d("Налоги и крючки", "tax.recheck_interval_s", K::Integer, L::Positive, "секунды", G, "Как часто перепроверять налоги всех монет (решение 30)"),
    d("Налоги и крючки", "tax.max_buy_pct", K::Decimal, L::Percent, "%", G, "Налог на покупку не больше — иначе монета не торгуется"),
    d("Налоги и крючки", "tax.max_sell_pct", K::Decimal, L::Percent, "%", G, "Налог на продажу не больше — иначе монета не торгуется"),
    d("Налоги и крючки", "tax.max_transfer_pct", K::Decimal, L::Percent, "%", G, "Налог на перевод не больше — иначе монета не торгуется"),
    d("Налоги и крючки", "check.probe_usdt", K::Decimal, L::Positive, "USDT", V, "Сумма пробного обмена (eth_call) при проверке налогов и крючков"),
    d("Налоги и крючки", "hooks.allowed_groups", K::Choice, L::None, "группы", G, "Какие группы крючков V4 допускаются: A / A+B / A+B+C (решение 32)").choices(&["A", "A+B", "A+B+C"]),
    d("Налоги и крючки", "hooks.max_impact_b_pct", K::Decimal, L::Percent, "%", G, "Допуск влияния крючка группы B на сумму обмена"),
    d("Налоги и крючки", "hooks.max_impact_c_pct", K::Decimal, L::Percent, "%", G, "Допуск влияния крючка группы C на сумму обмена"),
    d("Налоги и крючки", "hooks.recheck_interval_s", K::Integer, L::Positive, "секунды", G, "Как часто перепроверять влияние крючков B/C пробным обменом"),
    d("Возврат USDT", "usdt_return.mode", K::Choice, L::None, "режим", G, "Возврат USDT между биржей и кошельком: в каждой сделке / отдельно по порогу (решение 34)").choices(&["per_trade", "separate"]),
    d("Возврат USDT", "usdt_return.low_balance_usd", K::Decimal, L::Positive, "USDT", GV, "Режим «отдельно»: остаток USDT ниже — перегнать с другой стороны"),
    d("Возврат USDT", "usdt_return.amount_usd", K::Decimal, L::Positive, "USDT", GV, "Режим «отдельно»: сколько USDT перегонять за раз"),
    d("Застрявшие монеты", "stuck.mode", K::Choice, L::None, "режим", G, "Монета дошла, а продавать невыгодно: ожидание / порог убытка / перегон (решение 37)").choices(&["wait", "loss_limit", "relay"]),
    d("Застрявшие монеты", "stuck.wait_ms", K::Integer, L::Positive, "мс", G, "Режим «Ожидание»: ждать лучшей цены не дольше, затем продажа по рынку (максимум 3 ч = 10800000)").max(STUCK_WAIT_MAX_MS),
    d("Застрявшие монеты", "stuck.max_loss_pct", K::Decimal, L::Percent, "%", G, "Режим «Порог убытка»: убыток не больше — продать по рынку; больше — алерт с кнопками"),
    d("Застрявшие монеты", "stuck.max_relays", K::Integer, L::Positive, "раз", G, "Режим «Перегон»: сколько раз перегонять на лучшую площадку, затем продажа по рынку"),
    d("Монеты", "coin.blocked", K::Bool, L::None, "да/нет", C, "Ручной чёрный список: монета не торгуется (решение 36)"),
    d("Монеты", "coin.blocked_note", K::Text, L::None, "текст", C, "Причина ручного исключения монеты — для экрана «Монеты»"),
];
