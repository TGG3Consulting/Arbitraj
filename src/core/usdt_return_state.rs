//! Файл: core/usdt_return_state.rs
//! Что делает: состояние возврата выручки USDT закрытой сделки на площадку покупки (решение 138) — поле
//! Deal.usdt_return: идёт / вернулись / не вернулись. Плюс страховка: «USDT не вернулись» — журнал
//! (core.usdt_not_returned), событие Telegram usdt.not_returned и строка на экране «Сделки» (по полю сделки).
//! Кто вызывает: core/usdt_return_deal.rs (шаг после закрытия), core/ledger.rs (load — возврат, прерванный
//! перезапуском), core/manual_halted.rs («Продано вручную» — возврата нет по решению 100).
//! От чего зависит: deal.rs, engine.rs, ledger.rs (save), deps.rs (журнал, уведомления).
//! Настройки: нет (событие включается на экране уведомлений).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, lk};

/// Возврат выручки USDT сделки на площадку покупки (решение 138).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UsdtReturn {
    /// Перегон начат и ещё не закончился: сумма, откуда, куда.
    Pending {
        amount: Decimal,
        from: VenueId,
        to: VenueId,
    },
    /// Вернулись: отправлено amount, на площадке покупки зачислено credited.
    Done {
        amount: Decimal,
        credited: Decimal,
        from: VenueId,
        to: VenueId,
    },
    /// Не вернулись и бот сам не вернёт: сумма, где лежат, почему. Нужен перегон человеком.
    NotReturned {
        amount: Decimal,
        at: VenueId,
        why: String,
    },
}

/// Текст страховки «USDT не вернулись». Принимает: id сделки, сумму, где лежат, причину. Ошибок нет.
pub fn not_returned_text(deal: &str, amount: Decimal, at: &VenueId, why: &str) -> String {
    format!(
        "USDT не вернулись: сделка {deal}, {} лежат на {} — {why}. Перегоните на площадку покупки вручную",
        crate::notify::human::usd(amount),
        crate::notify::human::venue(&at.0)
    )
}

/// Страховка: журнал (Error, core.usdt_not_returned) и событие usdt.not_returned. Принимает: ядро, id сделки,
/// сумму, где лежат, причину. Ошибок нет.
pub async fn alarm(core: &Core, deal: &str, amount: Decimal, at: &VenueId, why: &str) {
    let text = not_returned_text(deal, amount, at, why);
    let deps = &core.inner.deps;
    deps.log(
        Level::Error,
        "core.usdt_not_returned",
        Some(&at.0),
        &text,
        None,
    )
    .await;
    // Решение 164: в Telegram — по-человечески, id сделки внизу.
    let tg = crate::notify::human::Msg::new(
        crate::notify::human::WARN,
        "USDT не вернулись на площадку покупки",
    )
    .kv(
        "Сумма",
        format!(
            "{} — лежат на {}",
            crate::notify::human::usd(amount),
            crate::notify::human::venue(&at.0)
        ),
    )
    .reason(why)
    .todo("перегнать USDT на площадку покупки вручную")
    .id(deal)
    .render();
    deps.notify("usdt.not_returned", Some(&at.0), &tg).await;
}

/// Ставит состояние возврата закрытой сделке в учёте (память, SQLite, интерфейс) с записью в хронологию.
/// Принимает: ядро, id сделки, состояние, текст для хронологии. Ошибок нет: сделки нет в учёте — ничего.
pub async fn set(core: &Core, deal: &str, state: UsdtReturn, note: &str) {
    let d = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(deal) else {
            return;
        };
        d.usdt_return = Some(state);
        d.note(note.to_string());
        d.clone()
    };
    super::ledger::save(core, &d).await;
}

/// Не вернулись: состояние сделки + страховка. Принимает: ядро, id сделки, сумму, где лежат, причину. Ошибок нет.
pub async fn mark_not_returned(core: &Core, deal: &str, amount: Decimal, at: &VenueId, why: &str) {
    let text = not_returned_text(deal, amount, at, why);
    let state = UsdtReturn::NotReturned {
        amount,
        at: at.clone(),
        why: why.to_string(),
    };
    set(core, deal, state, &text).await;
    alarm(core, deal, amount, at, why).await;
}

/// После перезапуска: возврат «идёт» у закрытой сделки — задача перегона потеряна вместе с памятью. Перевод мог уйти:
/// не повторяем, а ставим «не вернулись» со страховкой. Принимает: ядро, сделку (из базы). Возвращает: true —
/// состояние изменено (сохранить). Ошибок нет.
pub async fn on_restart(core: &Core, d: &mut Deal) -> bool {
    let Some(UsdtReturn::Pending { amount, from, .. }) = d.usdt_return.clone() else {
        return false;
    };
    let why = "возврат прерван перезапуском — проверьте на обеих площадках, ушёл ли перевод";
    d.note(not_returned_text(&d.id, amount, &from, why));
    d.usdt_return = Some(UsdtReturn::NotReturned {
        amount,
        at: from.clone(),
        why: why.to_string(),
    });
    alarm(core, &d.id, amount, &from, why).await;
    true
}
