//! Файл: core/usdt_return_state.rs
//! Что делает: состояние возврата выручки USDT закрытой сделки на площадку покупки (решение 138) — поле
//! Deal.usdt_return: идёт / ждёт повтора (решение 163) / вернулись / не вернулись / перегнано вручную (решение 163).
//! Плюс страховка: «USDT не вернулись» — журнал (core.usdt_not_returned), событие Telegram usdt.not_returned, строка
//! на экране «Сделки» (по полю сделки) и запись с кнопками на экране «Ручное управление» (решение 163,
//! core/manual_usdt.rs; в Telegram — событием manual.record).
//! Решение 227: состояние — у каждой ЧАСТИ возврата (часть = площадка, где лежат USDT; usdt_return_parts.rs): set,
//! mark_not_returned и on_restart меняют часть по её площадке, остальные части не трогают.
//! Решение 236: set вносит затраты части «вернулись» в итог сделки (deal_return_cost.rs) и отправляет отложенное
//! deal.closed, когда возврат закончен (deal_closed.rs).
//! Кто вызывает: core/usdt_return_deal.rs (шаг после закрытия), core/ledger.rs (load — возврат, прерванный
//! перезапуском), core/manual_halted.rs («Продано вручную» — возврата нет по решению 100), core/usdt_return_retry.rs
//! (повторы, решение 163), core/manual_usdt.rs («Перегнать», «Перегнано вручную»).
//! От чего зависит: deal.rs, engine.rs, ledger.rs (save), deps.rs (журнал, уведомления), deal_return_cost.rs, deal_closed.rs.
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
    /// Вернулись: запрошено к выводу amount, на площадке покупки зачислено credited. Решение 236: sent — ушло с
    /// площадки после округления до точности вывода (остаток amount − sent лежит на площадке-отправителе — не затрата);
    /// fee_usd — затраты вывода не в USDT (газ BNB кошелька DEX) в USDT; комиссия в USDT уже в sent − credited.
    /// None у обоих — запись до решения 236 (затраты возврата — amount − credited, газа в записи нет).
    Done {
        amount: Decimal,
        credited: Decimal,
        from: VenueId,
        to: VenueId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sent: Option<Decimal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fee_usd: Option<Decimal>,
    },
    /// Не вернулись и бот сам не вернёт: сумма, где лежат, почему. Нужен перегон человеком.
    NotReturned {
        amount: Decimal,
        at: VenueId,
        why: String,
    },
    /// Решение 163: перегон не состоялся, а прежний вывод точно не ушёл — ждёт повтора через usdt.return_retry_s.
    /// tries — сколько повторов уже сделано (первая попытка при закрытии не считается), why — причина последнего сбоя.
    /// Переживает перезапуск: задача повторов поднимается при запуске (usdt_return_retry::resume).
    Retrying {
        amount: Decimal,
        from: VenueId,
        to: VenueId,
        tries: u64,
        why: String,
    },
    /// Решение 163: «Перегнано вручную» — человек перевёл USDT сам; by — кто нажал (журнал и хронология).
    /// Решение 236, дополнение п. 2: credited — сколько зачислено на площадке покупки (ответ человека); затраты возврата
    /// части = amount − credited (deal_return_cost.rs). None — человек не знает (или запись до дополнения): затраты
    /// неизвестны, в итог не входят.
    Manual {
        amount: Decimal,
        at: VenueId,
        to: VenueId,
        by: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credited: Option<Decimal>,
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

/// Ставит состояние части возврата (по площадке состояния, решение 227) закрытой сделке в учёте (память, SQLite, интерфейс) с записью в хронологию.
/// Принимает: ядро, id сделки, состояние, текст для хронологии («» — без записи). Ошибок нет: сделки нет — ничего.
/// Решение 236: часть стала «вернулись» — её затраты вычитаются из итога сделки (deal_return_cost.rs: итог дня,
/// статистика монеты, журнал); возврат закончен — уходит отложенное уведомление deal.closed (deal_closed.rs).
pub async fn set(core: &Core, deal: &str, state: UsdtReturn, note: &str) {
    let (d, delta, notice) = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(deal) else {
            return;
        };
        // Решение 227: часть — по площадке состояния (первая или остальная).
        super::usdt_return_parts::put(d, state);
        if !note.is_empty() {
            d.note(note.to_string());
        }
        let delta = super::deal_return_cost::apply(d);
        let notice = super::deal_closed::take_ready(d);
        (d.clone(), delta, notice)
    };
    super::ledger::save(core, &d).await;
    super::deal_return_cost::after(core, &d, delta).await;
    if let Some(why) = notice {
        super::deal_closed::send(core, &d, &why).await;
    }
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

/// После перезапуска: часть возврата «идёт» у закрытой сделки — задача перегона потеряна вместе с памятью. Перевод мог
/// уйти: не повторяем, а ставим «не вернулись» со страховкой — каждой такой части (решение 227). Принимает: ядро,
/// сделку (из базы). Возвращает: true — состояние изменено (сохранить). Ошибок нет.
pub async fn on_restart(core: &Core, d: &mut Deal) -> bool {
    let lost: Vec<(Decimal, VenueId)> = super::usdt_return_parts::all(d)
        .into_iter()
        .filter_map(|p| match p {
            UsdtReturn::Pending { amount, from, .. } => Some((*amount, from.clone())),
            _ => None,
        })
        .collect();
    let why = "возврат прерван перезапуском — проверьте на обеих площадках, ушёл ли перевод";
    for (amount, from) in &lost {
        d.note(not_returned_text(&d.id, *amount, from, why));
        let state = UsdtReturn::NotReturned {
            amount: *amount,
            at: from.clone(),
            why: why.to_string(),
        };
        super::usdt_return_parts::put(d, state);
        alarm(core, &d.id, *amount, from, why).await;
    }
    !lost.is_empty()
}
