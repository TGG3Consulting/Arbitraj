//! Файл: settings/hint.rs
//! Что делает: решение 200 — режим подсказки имён сетей для кнопки «Проверить подключение» биржи без venue.bsc_network.
//! Внутри scope (только на время одного запроса справочника сетей монеты, gateway/network_hint.rs) незаданная
//! venue.bsc_network читается как пустое имя: адаптер разбирает справочник без сети BSC (ни одна сеть не совпадёт, адреса
//! контракта нет) и отдаёт имена всех сетей — из них Тигран выбирает имя BSC. Вне scope — как всегда: «не задана».
//! Кто вызывает: settings/read.rs (Store::text), gateway/network_hint.rs (scope).
//! От чего зависит: tokio (task_local).
//! Настройки: venue.bsc_network (только чтение в режиме подсказки).

/// Имя настройки, которая в режиме подсказки читается пустой.
pub const BSC_NETWORK: &str = "venue.bsc_network";

tokio::task_local! {
    /// Режим подсказки имён сетей (значение не используется — важен сам scope).
    pub static NETWORK_NAMES_HINT: ();
}

/// Идёт ли сейчас подсказка имён сетей и это та настройка. Ошибок нет.
pub fn empty_allowed(name: &str) -> bool {
    name == BSC_NETWORK && NETWORK_NAMES_HINT.try_with(|_| ()).is_ok()
}
