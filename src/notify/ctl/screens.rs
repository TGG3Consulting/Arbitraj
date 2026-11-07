//! Файл: notify/ctl/screens.rs
//! Что делает: экраны чата «Управление» (решение 176) без денег: «Главное меню» (с «📊 Отчёты» — решение 215), «Состояние» (режим, итог дня по
//! площадкам — решение 173, паузы, карантин; кнопки Старт / Пауза / Стоп, «Снять паузу», «Снять карантин», «Сбросить
//! лимит дня» общий и по площадке — те же кнопки «c:…», что у меню /menu «Общего», решение 157) и «Ручное управление»
//! (сколько записей ждут и ссылка на чат, где их кнопки). Только рендер — данные даёт вызывающий.
//! Кто вызывает: notify/ctl/press.rs, notify/ctl/live.rs.
//! От чего зависит: notify/menu.rs, menu_day.rs (тексты и кнопки плашки), ctl/kb.rs, core/risk.rs, core/types.
//! Настройки: нет.

use crate::core::commands_control::label;
use crate::core::risk::Risk;
use crate::core::types::{ManualRecord, RecordKind};

use super::super::{menu, menu_day};
use super::kb::{Screen, btn, link};

/// «Главное меню». Принимает: Risk. Ошибок нет.
pub fn root(r: &Risk) -> Screen {
    Screen::new(format!(
        "Управление ARB — режим «{}». Выберите действие.",
        label(r.mode)
    ))
    .row(vec![btn("Состояние", "k:st"), btn("Балансы", "k:bal")])
    .row(vec![
        btn("Перевести USDT", "k:tr"),
        btn("Ручное управление", "k:man"),
    ])
    // Решение 215: отчёты Excel файлом в этот чат (notify/ctl/report.rs).
    .one(btn("📊 Отчёты", "r:m"))
    .one(btn("Обновить", "k:root"))
}

/// «Состояние»: текст плашки режима и все её кнопки. Принимает: Risk, сколько записей ждут действия. Ошибок нет.
pub fn state(r: &Risk, waiting: usize) -> Screen {
    let (text, _) = menu::state(r, waiting);
    let mut s = Screen::new(text);
    s.rows.push(menu::mode_buttons(r));
    s.rows.retain(|x| !x.is_empty());
    let (_, venue) = menu::venue_buttons(r);
    let (_, day) = menu_day::day(r);
    for b in venue.into_iter().chain(day) {
        s.rows.push(vec![b]);
    }
    s.one(btn("Обновить", "k:st")).nav(Some("k:root"))
}

/// Ссылка на сообщение супергруппы: t.me/c/<id без -100>/<сообщение> (ссылки на сообщения закрытых групп,
/// https://core.telegram.org/api/links#message-links). Принимает: id чата, id сообщения.
/// Возвращает: адрес или None — чат не супергруппа (у обычной группы ссылок на сообщения нет). Ошибок нет.
pub fn message_link(chat: i64, msg: i64) -> Option<String> {
    let s = chat.to_string();
    let id = s.strip_prefix("-100")?;
    Some(format!("https://t.me/c/{id}/{msg}"))
}

/// «Ручное управление»: сколько записей ждут, по видам, и где их кнопки. Принимает: ждущие записи, название чата с
/// кнопками («Неудачные» или «Общий»), ссылку на последнее сообщение там (None — нет). Ошибок нет.
pub fn manual(waiting: &[&ManualRecord], chat: &str, last: Option<String>) -> Screen {
    let mut s = if waiting.is_empty() {
        Screen::new("Ручное управление: записей, ждущих действия, нет.")
    } else {
        Screen::new(format!(
            "Ручное управление: ждут действия — {}",
            waiting.len()
        ))
    };
    for k in [
        RecordKind::Alert,
        RecordKind::Halted,
        RecordKind::UsdtReturn,
        RecordKind::Holding,
    ] {
        let n = waiting.iter().filter(|r| r.kind == k).count();
        if n > 0 {
            s = s.line(format!("• {}: {n}", k.title()));
        }
    }
    if !waiting.is_empty() {
        s = s.line(format!(
            "Кнопки «Продать сейчас», «Продать и перегнать», «Продано вручную» — в чате «{chat}» или здесь: «Обработать»."
        ));
        // Решение 192: карточка первой ждущей записи со всеми её кнопками и «Предыдущая» / «Следующая».
        s = s.one(btn("Обработать", &super::card::code(0, &waiting[0].key)));
    }
    if let Some(url) = last {
        s.rows
            .push(vec![link(&format!("Открыть в чате «{chat}»"), &url)]);
    }
    s.one(btn("Обновить", "k:man")).nav(Some("k:root"))
}
