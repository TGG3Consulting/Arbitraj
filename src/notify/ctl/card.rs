//! Файл: notify/ctl/card.rs
//! Что делает: решение 192 (Тигран, 06.11.2026) — карточка проблемной сделки в чате «Управление»: кнопка «Обработать»
//! на экране «Ручное управление» открывает первую ждущую запись; на карточке — текст записи и ВСЕ её кнопки (те же,
//! что в чате «Неудачные» и на экране «Ручное управление» интерфейса: нажатие → «Да, …» → та же команда ядра,
//! notify/press_record.rs), плюс «◀ Предыдущая» / «Следующая ▶» по ждущим записям. Запись уже обработана (её нет среди
//! ждущих или она выполнена из Telegram) — на карточке «обработано» и активны только «Предыдущая» / «Следующая».
//! callback_data карточки — «k:mc.<позиция>.<ключ записи>»: позиция нужна, чтобы у обработанной (ушедшей из списка)
//! записи найти соседей.
//! Кто вызывает: notify/ctl/view.rs (render), notify/ctl/screens.rs (кнопка «Обработать»).
//! От чего зависит: notify/mirror_view.rs (кнопки записи), ctl/kb.rs, core/types (ManualRecord).
//! Настройки: нет.

use serde_json::Value;

use crate::core::types::ManualRecord;

use super::super::mirror_view::buttons_for;
use super::kb::{Screen, btn};

/// Сосед карточки: позиция и ключ записи.
pub type Near = Option<(usize, String)>;

/// callback_data карточки записи на позиции `idx`. Ошибок нет.
pub fn code(idx: usize, key: &str) -> String {
    format!("k:mc.{idx}.{key}")
}

/// Разбор «<позиция>.<ключ>» из callback_data. None — не карточка. Ошибок нет.
pub fn parse(rest: &str) -> Option<(usize, String)> {
    let (i, key) = rest.split_once('.')?;
    let idx = i.parse().ok()?;
    (!key.is_empty()).then(|| (idx, key.to_string()))
}

/// Соседи карточки: (предыдущая, следующая) как (позиция, ключ). Запись в списке — её соседи; ушла из списка — на её
/// позиции теперь следующая запись, предыдущая — перед ней. Принимает: ключи ждущих по порядку, позицию, ключ.
/// Ошибок нет.
pub fn neighbours(
    keys: &[String],
    idx: usize,
    key: &str,
) -> (Near, Near) {
    let at = |i: usize| keys.get(i).map(|k| (i, k.clone()));
    match keys.iter().position(|k| k == key) {
        Some(p) => (p.checked_sub(1).and_then(at), at(p + 1)),
        None => {
            let p = idx.min(keys.len());
            (p.checked_sub(1).and_then(at), at(p))
        }
    }
}

/// Карточка. Принимает: ждущие записи по порядку, позицию и ключ записи. Ошибок нет.
pub fn card(waiting: &[&ManualRecord], idx: usize, key: &str) -> Screen {
    let keys: Vec<String> = waiting.iter().map(|r| r.key.clone()).collect();
    let (prev, next) = neighbours(&keys, idx, key);
    let mut s = match waiting.iter().position(|r| r.key == key) {
        Some(p) => {
            let r = waiting[p];
            let (btns, notes): (Vec<Value>, Vec<String>) = buttons_for(r, "", true);
            let mut s = Screen::new(format!(
                "Проблемная сделка {} из {} — {}\n{}",
                p + 1,
                waiting.len(),
                r.kind.title(),
                r.text
            ));
            for n in notes {
                s = s.line(n);
            }
            for b in btns {
                s.rows.push(vec![b]);
            }
            s
        }
        None => Screen::new(format!(
            "✅ Обработано: {key}\nЗаписи больше нет среди ждущих действия. Ждут — {}.",
            waiting.len()
        )),
    };
    let nav =
        |label: &str, t: Option<(usize, String)>| t.and_then(|(i, k)| btn(label, &code(i, &k)));
    s = s.row(vec![nav("◀ Предыдущая", prev), nav("Следующая ▶", next)]);
    s.one(btn("Обновить", &code(idx, key))).nav(Some("k:man"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn neighbours_in_list_and_after_done() {
        let keys = k(&["a.1", "a.2", "a.3"]);
        assert_eq!(
            neighbours(&keys, 1, "a.2"),
            (Some((0, "a.1".into())), Some((2, "a.3".into())))
        );
        assert_eq!(neighbours(&keys, 0, "a.1").0, None);
        // a.2 обработана и ушла: на её позиции 1 теперь a.3.
        let left = k(&["a.1", "a.3"]);
        assert_eq!(
            neighbours(&left, 1, "a.2"),
            (Some((0, "a.1".into())), Some((1, "a.3".into())))
        );
        assert_eq!(
            parse("3.a.20261106REAL"),
            Some((3, "a.20261106REAL".into()))
        );
    }
}
