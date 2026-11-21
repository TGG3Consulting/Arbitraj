//! Файл: venues/bsc_dex/pools/family_intern.rs
//! Что делает: решение 409 (t194) — таблица видов DEX (интернирование Family): одна запись на сочетание «id, протокол,
//! graphable, tradable, fork, вид шага исполнителя», живёт всю работу процесса (&'static). Чтение — снимок таблицы за
//! ArcSwap без замка и без выделения строки (поиск по &str): families::make вызывается на горячем пути котировки на
//! каждый пул (quote/select.rs → families::still → classify → make), и прежний глобальный Mutex на каждый вызов давал
//! борьбу потоков за замок (perf 18.11: Mutex::lock_contended ← families::make). Новая запись (только при новом
//! сочетании — при старте и смене dex.kind) — под замком писателя: повторный поиск, копия таблицы, новый снимок.
//! Поведение то же: то же сочетание → тот же &'static Family (std::ptr::eq в dex_rules::still).
//! Кто вызывает: pools/families.rs (make).
//! От чего зависит: pools/families.rs (Family), arc-swap.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use arc_swap::ArcSwap;

use super::families::Family;

/// id DEX → записи видов этого id (разные сочетания протокола и признаков; обычно одна).
type Table = HashMap<String, Vec<&'static Family>>;

/// Текущий снимок таблицы — читается без замка.
static TABLE: LazyLock<ArcSwap<Table>> = LazyLock::new(|| ArcSwap::from_pointee(HashMap::new()));
/// Писатели таблицы — строго по одному (одна запись на сочетание).
static WRITE: Mutex<()> = Mutex::new(());

/// Что делает: запись вида для id, подходящая под `same`; нет — создаёт её из `build` (под замком писателя).
/// Принимает: id DEX, условие совпадения сочетания, сборку нового вида (получает id с жизнью процесса).
/// Возвращает: вид с жизнью процесса. Ошибок нет (отравленный замок — продолжаем: под ним нет данных).
pub fn intern(
    id: &str,
    same: impl Fn(&Family) -> bool,
    build: impl FnOnce(&'static str) -> Family,
) -> &'static Family {
    let find = |t: &Table| t.get(id).and_then(|v| v.iter().copied().find(|&f| same(f)));
    if let Some(f) = find(&TABLE.load()) {
        return f;
    }
    let _turn = WRITE.lock().unwrap_or_else(|p| p.into_inner());
    let cur = TABLE.load_full();
    // Пока ждали замок, ту же запись мог создать другой поток — берём её (одна запись на сочетание).
    if let Some(f) = find(&cur) {
        return f;
    }
    let leaked: &'static str = Box::leak(id.to_string().into_boxed_str());
    let f: &'static Family = Box::leak(Box::new(build(leaked)));
    let mut next = (*cur).clone();
    next.entry(id.to_string()).or_default().push(f);
    TABLE.store(Arc::new(next));
    f
}
