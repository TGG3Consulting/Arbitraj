//! Файл: notify/acts_e.rs
//! Что делает: решение 408 (t193) — кнопки действий тревог «e:<что>.<данные>» (путь notify::acts::e; вынесено из
//! acts.rs — размер файла). Данные — только «что и где»; остальное читает нажатие (notify/press_event.rs,
//! press_event_money.rs). Кошелька нет — «-». Предел 64 байта проверяет acts::keyboard при сборке клавиатуры.
//! Кто вызывает: core (места тревог), notify/menu_waits.rs, notify/mirror_view.rs.
//! От чего зависит: notify/acts.rs (Act, act), notify/human.rs (имя площадки).
//! Настройки: нет.

use super::acts::{Act, act};

/// «Снять очередь кошелька» (send.queue_halted) — e:qh.<площадка>.<кошелёк или ->.
pub fn release_queue(venue: &str, wallet: Option<&str>, who: &str) -> Act {
    act(
        format!("Снять очередь {who}"),
        format!("e:qh.{venue}.{}", wallet.unwrap_or("-")),
    )
}

/// «Отменить ожидание» ручного действия (решение 405) — e:wc.<ID ожидания>.
pub fn cancel_wait(id: &str, what: &str, coin: &str) -> Act {
    act(
        format!("Отменить ожидание «{what}» {coin}"),
        format!("e:wc.{id}"),
    )
}

/// «Открыть сделку <ID>» — e:od.<ID сделки>.
pub fn open_deal(id: &str) -> Act {
    act(format!("Открыть сделку {id}"), format!("e:od.{id}"))
}

/// Перевод USDT с заполненными откуда / куда / сколько — e:tu.<откуда>.<куда>.<сумма>.
pub fn transfer(from: &str, to: &str, amount: &str, title: &str) -> Act {
    act(title, format!("e:tu.{from}.{to}.{amount}"))
}

/// «Перевести USDT сюда» — выбор «Откуда» при выбранном «Куда» — e:tt.<куда>.
pub fn transfer_to(to: &str, label: &str) -> Act {
    act(format!("Перевести USDT на {label}"), format!("e:tt.{to}"))
}

/// «Продать» (SellBalance, торговый счёт биржи) — e:sb.<биржа>.<тикер биржи>.
pub fn sell(venue: &str, ticker: &str) -> Act {
    act(
        format!("Продать {ticker} на {}", super::human::venue(venue)),
        format!("e:sb.{venue}.{ticker}"),
    )
}

/// «Развернуть исполнитель» кошелька — e:xd.<площадка>.<кошелёк>. Нажимает только человек.
pub fn deploy_executor(venue: &str, wallet: &str, name: &str) -> Act {
    act(
        format!("Развернуть исполнитель «{name}»"),
        format!("e:xd.{venue}.{wallet}"),
    )
}

/// «Открыть вход монеты» с биржи (coin.entry_blocked_venues) — e:oe.<биржа>.<id монеты>.
pub fn open_entry(venue: &str, coin: &str, label: &str) -> Act {
    act(
        format!("Открыть вход {label} с {}", super::human::venue(venue)),
        format!("e:oe.{venue}.{coin}"),
    )
}

/// «Убрать из чёрного списка» (coin.blocked) — e:ub.<id монеты>.
pub fn unblacklist(coin: &str, label: &str) -> Act {
    act(
        format!("Убрать {label} из чёрного списка"),
        format!("e:ub.{coin}"),
    )
}
