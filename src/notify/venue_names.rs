//! Файл: notify/venue_names.rs
//! Что делает: решение 432 (шаг 4 двух сетей) — имена площадок DEX для человека, которые ставит ядро при запуске
//! (core/chain_run.rs publish_names): одна сеть площадок DEX — «DEX» (как раньше), сетей несколько — «DEX <сеть>»
//! (dex.chain площадки). notify/human.rs venue() берёт имя отсюда, а таблицу имён адаптеров — запасной (без ядра:
//! утилиты, тесты). Карточка сделки (core/deal_card.rs) по этому же списку понимает, что площадка — DEX (кошелёк в пути).
//! Кто вызывает: core/chain_run.rs (set_dex), notify/human.rs (dex_name).
//! От чего зависит: ничего.
//! Настройки: нет (сеть — dex.chain, читает ядро).

use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};

/// id площадки DEX → имя для человека (последний запуск ядра).
static DEX: LazyLock<RwLock<BTreeMap<String, String>>> = LazyLock::new(Default::default);

/// Что делает: заменяет имена площадок DEX. Принимает: (id, имя). Ошибок нет.
pub fn set_dex(names: impl IntoIterator<Item = (String, String)>) {
    *DEX.write().unwrap_or_else(|p| p.into_inner()) = names.into_iter().collect();
}

/// Что делает: имя площадки DEX, поставленное ядром. Принимает: id. Возвращает: None — ядро такую DEX не сообщало.
/// Ошибок нет.
pub fn dex_name(id: &str) -> Option<String> {
    DEX.read()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .cloned()
}
