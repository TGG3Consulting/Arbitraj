//! Файл: notify/alert_card.rs
//! Что делает: решение 251 (Тигран 10.11: согласовано) — карточка сделки для шаблона алерта (notify/alert.rs): монета с
//! коротким адресом, путь сделки («BingX → DEX (кошелёк w2) → BingX (USDT)») и где сейчас монета или деньги. Ядро
//! ставит поиск карточки по id сделки при запуске (install, core/engine.rs); шаблон перед отправкой в Telegram берёт
//! свежую карточку по строке «Сделка: <id>» и вставляет строки «Монета», «Путь», «Где сейчас», которых в тексте нет
//! (enrich). Так путь виден во всех алертах о сделке, даже где ядро передало только id.
//! Кто вызывает: core/engine.rs (install), core/deal_text.rs (собирает DealCard), notify/alert.rs (lookup, enrich).
//! От чего зависит: ничего (карточку собирает ядро из учёта сделки — только факты учёта).
//! Настройки: нет.

use std::sync::{Arc, RwLock};

/// Карточка сделки — готовые строки для человека (собирает core/deal_text.rs::card из учёта сделки).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DealCard {
    /// «PIG (0x1234…abcd)»; адреса нет — только тикер.
    pub coin: String,
    /// «BingX → DEX (кошелёк w2) → BingX (USDT)».
    pub path: String,
    /// «монета: 1234 PIG на DEX (кошелёк w2)» / «в пути: перевод с BingX» / «монет нет — сделка закрыта».
    pub now: String,
}

/// Подписи строк карточки в тексте алерта (порядок — как в шаблоне).
pub const COIN: &str = "Монета";
pub const PATH: &str = "Путь";
pub const NOW: &str = "Где сейчас";
/// Подпись строки «где монеты» в записях экрана (core/manual_rows.rs) — при вставленной «Где сейчас» убирается.
pub const COINS: &str = "Монеты";
/// Подпись строки id сделки (Msg::id, notify/human_msg.rs).
pub const DEAL: &str = "Сделка";

/// Поиск карточки по id сделки.
type Lookup = Arc<dyn Fn(&str) -> Option<DealCard> + Send + Sync>;

/// Поиск, поставленный ядром (None — ядро не запущено: карточек нет, текст уходит как есть).
static LOOKUP: RwLock<Option<Lookup>> = RwLock::new(None);

/// Ставит поиск карточки (ядро при запуске; повторный вызов заменяет прежний). Принимает: функцию. Ошибок нет.
pub fn install(f: impl Fn(&str) -> Option<DealCard> + Send + Sync + 'static) {
    let mut g = LOOKUP.write().unwrap_or_else(|p| p.into_inner());
    *g = Some(Arc::new(f));
}

/// Карточка сделки по id из учёта ядра. Принимает: id. Возвращает: карточку или None (нет ядра или сделки).
/// Важно: вызывать вне замков ядра (notify шлёт из своих задач — замков ядра там нет).
pub fn lookup(id: &str) -> Option<DealCard> {
    let f = LOOKUP.read().unwrap_or_else(|p| p.into_inner()).clone()?;
    f(id)
}

/// id сделки из строки «Сделка: <id>» текста алерта (первая такая строка). Ошибок нет.
pub fn deal_id(plain: &str) -> Option<&str> {
    let p = format!("{DEAL}: ");
    plain
        .lines()
        .find_map(|l| l.strip_prefix(p.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Вставляет строки карточки («Монета», «Путь», «Где сейчас») сразу после первой строки «Сделка: …», если такой
/// подписи в тексте ещё нет (вставлена «Где сейчас» — строка «Монеты: …» убирается как повтор). Нет строки
/// «Сделка» — текст как есть. Принимает: текст алерта, карточку.
/// Возвращает: текст. Ошибок нет.
pub fn enrich(plain: &str, card: &DealCard) -> String {
    let has = |label: &str| plain.lines().any(|l| l.starts_with(&format!("{label}: ")));
    let extra: Vec<String> = [(COIN, &card.coin), (PATH, &card.path), (NOW, &card.now)]
        .into_iter()
        .filter(|(l, v)| !v.is_empty() && !has(l))
        .map(|(l, v)| format!("{l}: {v}"))
        .collect();
    if extra.is_empty() {
        return plain.to_string();
    }
    let p = format!("{DEAL}: ");
    let now_added = extra.iter().any(|l| l.starts_with(&format!("{NOW}: ")));
    let mut out = Vec::new();
    let mut done = false;
    for l in plain.lines() {
        // Строка «Монеты: N на площадке» (записи экрана) повторяет «Где сейчас» — не дублируем.
        if now_added && l.starts_with(&format!("{COINS}: ")) {
            continue;
        }
        out.push(l.to_string());
        if !done && l.starts_with(p.as_str()) {
            out.extend(extra.iter().cloned());
            done = true;
        }
    }
    out.join("\n")
}
