//! Файл: core/manual_usdt.rs
//! Что делает: «USDT не вернулись» на экране «Ручное управление» (решение 163, Тигран 05.11.2026): закрытая сделка,
//! чья выручка USDT не вернулась на площадку покупки (Deal.usdt_return = NotReturned: все повторы неудачны, исход
//! неизвестен, повторов нет), — запись с кнопками «Перегнать» (ещё одна попытка возврата той же командой ядра —
//! usdt_return_attempt::attempt: свободные USDT площадки, не больше выручки) и «Перегнано вручную» (человек перевёл
//! сам: возврат помечается «вручную», в журнал — кто нажал). Кнопки «Продать» нет — уже продано. В Telegram запись
//! уходит тем же зеркалом экрана (событие manual.record, notify/mirror*.rs, решение 156). Повторное нажатие по уже
//! решённой записи — «проигнорирована», действие не повторяется.
//! Кто вызывает: core/manual_feed.rs (записи), core/commands.rs (команды UsdtRetry, UsdtReturnedManually),
//! core/manual_return.rs (Core::manual — таблица экрана; список сделок — Core::usdt_not_returned в manual_feed.rs).
//! От чего зависит: usdt_return_attempt.rs, usdt_return_state.rs, manual_rows.rs (btn), deal.rs, ledger.rs.
//! Настройки: нет (повторы — usdt.return_retry_s, usdt.return_retries_max в usdt_return_retry.rs).

use rust_decimal::Decimal;

use crate::core::types::{ManualAction as A, ManualRecord, RecordKind, VenueId};
use crate::gateway::urgency::{self, Urgency};
use crate::notify::human as hu;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, lk};
use super::manual_rows::btn;
use super::usdt_return_state::{self as st, UsdtReturn};

/// Запись «USDT не вернулись» с кнопками «Перегнать» и «Перегнано вручную». Принимает: сделку.
/// Возвращает: запись или None — у сделки возврат не в состоянии «не вернулись». Ошибок нет.
pub fn usdt_record(d: &Deal) -> Option<ManualRecord> {
    let Some(UsdtReturn::NotReturned { amount, at, why }) = &d.usdt_return else {
        return None;
    };
    let (id, v, to, sum) = (&d.id, &at.0, &d.direction.buy_on.0, amount.round_dp(4));
    // «Продано вручную» (решение 100): бот не знает, где USDT, — перегон только человеком.
    let skip = d
        .usdt_return_skip
        .as_ref()
        .map(|w| format!("бот выручку этой сделки не перегоняет: {w}"));
    let same = (v == to).then(|| format!("USDT уже на площадке покупки {to}"));
    Some(ManualRecord {
        key: ManualRecord::key_of(RecordKind::UsdtReturn, id),
        kind: RecordKind::UsdtReturn,
        object_id: id.clone(),
        deal_id: id.clone(),
        venue: v.clone(),
        // Решение 164: человеческий текст записи (тот же уходит в Telegram зеркалом экрана).
        text: hu::Msg::new(hu::STOP, "USDT не вернулись на биржу")
            .line(format!("{} {}", hu::usd(*amount), hu::route(v, to)))
            .kv("Лежат на", hu::venue(v))
            .reason(why)
            .todo("«Перегнать» или, если перевели сами, «Перегнано вручную»")
            .id(id)
            .render(),
        reason: crate::notify::reason::short(why),
        reason_full: why.clone(),
        buttons: vec![
            btn(
                "usdt_retry",
                "Перегнать",
                A::UsdtRetry {
                    deal_id: id.clone(),
                },
                format!(
                    "Перегнать USDT сделки {id} с {v} на {to}? Сумма — свободные USDT на {v}, не больше {sum}. Если причина — «исход неизвестен» или «вывод мог уйти», сначала проверьте обе площадки: перевод мог уже уйти."
                ),
                skip.or(same),
                false,
            ),
            btn(
                "usdt_manual",
                "Перегнано вручную",
                A::UsdtReturnedManually {
                    deal_id: id.clone(),
                },
                format!(
                    "Отметить, что {sum} USDT сделки {id} вы перевели с {v} на {to} сами? Запись закроется, бот этот возврат больше не делает."
                ),
                None,
                true,
            ),
        ],
    })
}

/// Команды записи «USDT не вернулись». Принимает: ядро, действие, кто (commands::who). Возвращает: None — действие
/// не отсюда; Some(итог). Запись уже решена (повторное нажатие) — Ok «проигнорирована», действие не повторяется.
pub async fn command(core: &Core, a: &A, who: &str) -> Option<Result<String, String>> {
    let (deal, retry) = match a {
        A::UsdtRetry { deal_id } => (deal_id, true),
        A::UsdtReturnedManually { deal_id } => (deal_id, false),
        _ => return None,
    };
    Some(match take(core, deal, who, retry).await {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Ok(format!(
            "запись «USDT не вернулись» сделки {deal} уже решена или не существует — команда проигнорирована"
        )),
        Err(e) => Err(e),
    })
}

/// Под замком учёта: «не вернулись» → «идёт» («Перегнать», попытка — отдельной задачей) или → «вручную».
/// Возвращает: Ok(Some(текст)) — выполнено; Ok(None) — записи нет; Err — кнопка недоступна (причина).
async fn take(core: &Core, deal: &str, who: &str, retry: bool) -> Result<Option<String>, String> {
    let (d, amount, at, to) = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(deal) else {
            return Ok(None);
        };
        let Some(UsdtReturn::NotReturned { amount, at, .. }) = d.usdt_return.clone() else {
            return Ok(None);
        };
        let to = d.direction.buy_on.clone();
        if retry {
            if let Some(w) = &d.usdt_return_skip {
                return Err(format!("бот выручку этой сделки не перегоняет: {w}"));
            }
            if at == to {
                return Err(format!("USDT уже на площадке покупки {}", to.0));
            }
            d.usdt_return = Some(UsdtReturn::Pending {
                amount,
                from: at.clone(),
                to: to.clone(),
            });
            d.note(format!(
                "«Перегнать» ({who}): ещё одна попытка возврата USDT {} → {}",
                at.0, to.0
            ));
        } else {
            d.usdt_return = Some(UsdtReturn::Manual {
                amount,
                at: at.clone(),
                to: to.clone(),
                by: who.to_string(),
            });
            d.note(format!(
                "Перегнано вручную ({who}): {amount} USDT {} → {} — человек перевёл сам, запись «USDT не вернулись» закрыта",
                at.0, to.0
            ));
        }
        (d.clone(), amount, at, to)
    };
    super::ledger::save(core, &d).await;
    let text = if retry {
        start(core, deal, amount, at.clone(), to.clone(), who);
        format!(
            "Сделка {deal}: перегон USDT {} → {} запущен (свободные USDT, не больше {})",
            at.0,
            to.0,
            amount.round_dp(4)
        )
    } else {
        format!(
            "Сделка {deal}: возврат USDT отмечен «перегнано вручную» ({who}), {} USDT {} → {}",
            amount.round_dp(4),
            at.0,
            to.0
        )
    };
    core.inner
        .deps
        .log(Level::Info, "core.usdt_return", Some(&at.0), &text, None)
        .await;
    Ok(Some(text))
}

/// «Перегнать»: одна попытка отдельной задачей класса Trade; итог — в состояние сделки: вернулись или снова
/// «не вернулись» (запись вернётся на экран и в Telegram). Ошибок нет.
fn start(core: &Core, deal: &str, cap: Decimal, at: VenueId, to: VenueId, who: &str) {
    let (c, d, who) = (core.clone(), deal.to_string(), who.to_string());
    let tag = format!(
        "m{}",
        chrono::Utc::now().timestamp_millis().rem_euclid(1_000_000)
    );
    // Решение 177: ручная команда — сбой перегона не «ошибка площадки подряд» (risk_count.rs).
    tokio::spawn(urgency::scope(
        Urgency::Trade,
        super::risk_count::manual(async move {
            match super::usdt_return_attempt::attempt(&c, &d, cap, (&at, &to), &tag).await {
                Ok((sent, credited)) => {
                    let t = format!(
                        "USDT вернулись («Перегнать», {who}): {sent} {} → {}, зачислено {credited}",
                        at.0, to.0
                    );
                    let state = UsdtReturn::Done {
                        amount: sent,
                        credited,
                        from: at,
                        to,
                    };
                    st::set(&c, &d, state, &t).await;
                }
                Err(e) => {
                    let why = if e.outcome_unknown() {
                        format!(
                            "«Перегнать»: исход перегона неизвестен ({e}) — проверьте обе площадки, перевод мог уйти"
                        )
                    } else {
                        format!("«Перегнать»: перегон не состоялся: {e}")
                    };
                    st::mark_not_returned(&c, &d, cap, &at, &why).await;
                }
            }
        }),
    ));
}
