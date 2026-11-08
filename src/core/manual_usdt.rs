//! Файл: core/manual_usdt.rs
//! Что делает: «USDT не вернулись» на экране «Ручное управление» (решение 163, Тигран 05.11.2026): закрытая сделка,
//! чья выручка USDT не вернулась на площадку покупки (Deal.usdt_return = NotReturned: все повторы неудачны, исход
//! неизвестен, повторов нет), — запись с кнопками «Перегнать» (ещё одна попытка возврата той же командой ядра —
//! usdt_return_attempt::attempt: сумма части) и «Перегнано вручную» (человек перевёл
//! сам: возврат помечается «вручную», в журнал — кто нажал). Кнопки «Продать» нет — уже продано. В Telegram запись
//! уходит тем же зеркалом экрана (событие manual.record, notify/mirror*.rs, решение 156). Повторное нажатие по уже
//! решённой записи — «проигнорирована», действие не повторяется.
//! Решение 224: у «Продано вручную» «Перегнать» доступна, если в сделке были продажи бота (перегон — не больше их
//! выручки); недоступна — текст записи «Перегнать» не предлагает, только «Перегнано вручную».
//! Решение 227: запись — на каждую ЧАСТЬ возврата «не вернулись» (часть = площадка, где лежат USDT); «Перегнать» —
//! ровно сумма части (не свободный баланс), свободно меньше — попытка ничего не отправляет (кроме нехватки в пределах
//! допуска usdt.return_tolerance_usd — тогда свободный остаток, usdt_return_attempt.rs).
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

/// Записи «USDT не вернулись» сделки — по одной на каждую часть возврата в этом состоянии (решение 227). Принимает:
/// сделку. Возвращает: записи (пусто — ни одной такой части). Ошибок нет.
pub fn usdt_records(d: &Deal) -> Vec<ManualRecord> {
    super::usdt_return_parts::all(d)
        .into_iter()
        .filter_map(|p| usdt_record(d, p))
        .collect()
}

/// Запись «USDT не вернулись» части с кнопками «Перегнать» и «Перегнано вручную». Решение 227: ключ записи и поле
/// deal_id команд — ключ части (usdt_return_parts::key: первая часть — id сделки, остальные — «<id>@<площадка>»).
/// Принимает: сделку, часть. Возвращает: запись или None — часть не в состоянии «не вернулись». Ошибок нет.
fn usdt_record(d: &Deal, part: &UsdtReturn) -> Option<ManualRecord> {
    let UsdtReturn::NotReturned { amount, at, why } = part else {
        return None;
    };
    let key = super::usdt_return_parts::key(d, at);
    let (id, v, to, sum) = (&d.id, &at.0, &d.direction.buy_on.0, amount.round_dp(4));
    // «Продано вручную» (решение 100): выручку человека перегоняет только человек; решение 224 — выручку продаж бота
    // «Перегнать» перегоняет (retry_block).
    let block = retry_block(d, at, &d.direction.buy_on);
    // Решение 224: «Перегнать» недоступна — текст её не предлагает, только доступное.
    let todo = if block.is_some() {
        "перевести USDT на площадку покупки самим, затем нажать «Перегнано вручную»"
    } else {
        "«Перегнать» или, если перевели сами, «Перегнано вручную»"
    };
    Some(ManualRecord {
        key: ManualRecord::key_of(RecordKind::UsdtReturn, &key),
        kind: RecordKind::UsdtReturn,
        object_id: key.clone(),
        deal_id: id.clone(),
        venue: v.clone(),
        // Решение 164: человеческий текст записи (тот же уходит в Telegram зеркалом экрана).
        text: hu::Msg::new(hu::STOP, "USDT не вернулись на биржу")
            .line(format!("{} {}", hu::usd(*amount), hu::route(v, to)))
            .kv("Лежат на", hu::venue(v))
            .reason(why)
            .todo(todo)
            .id(id)
            .render(),
        reason: crate::notify::reason::short(why),
        reason_full: why.clone(),
        buttons: vec![
            btn(
                "usdt_retry",
                "Перегнать",
                A::UsdtRetry {
                    deal_id: key.clone(),
                },
                format!(
                    "Перегнать USDT сделки {id} с {v} на {to}? Сумма — {sum} (выручка этой части; свободно меньше — перегон не отправится). Если причина — «исход неизвестен» или «вывод мог уйти», сначала проверьте обе площадки: перевод мог уже уйти."
                ),
                block,
                false,
            ),
            btn(
                "usdt_manual",
                "Перегнано вручную",
                A::UsdtReturnedManually {
                    deal_id: key.clone(),
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

/// Почему «Перегнать» недоступна. «Продано вручную» (решение 100) без продаж бота на площадке этой части — бот
/// выручку не перегоняет; с продажами бота — доступна (решение 224, перегон не больше их выручки на этой площадке,
/// решение 227). USDT уже на площадке покупки — тоже недоступна. Принимает: сделку, где лежат USDT, площадку покупки.
/// Возвращает: причину или None. Ошибок нет.
pub fn retry_block(d: &Deal, at: &VenueId, to: &VenueId) -> Option<String> {
    if let Some(w) = &d.usdt_return_skip
        && super::usdt_return_parts::sold_at(d, at) <= Decimal::ZERO
    {
        return Some(format!("бот выручку этой сделки не перегоняет: {w}"));
    }
    (at == to).then(|| format!("USDT уже на площадке покупки {}", to.0))
}

/// Команды записи «USDT не вернулись». Принимает: ядро, действие, кто (commands::who). Возвращает: None — действие
/// не отсюда; Some(итог). Запись уже решена (повторное нажатие) — Ok «проигнорирована», действие не повторяется.
pub async fn command(core: &Core, a: &A, who: &str) -> Option<Result<String, String>> {
    let (deal, retry) = match a {
        A::UsdtRetry { deal_id } => (deal_id, true),
        A::UsdtReturnedManually { deal_id } => (deal_id, false),
        _ => return None,
    };
    // Решение 227: deal_id команды — ключ части («<id>» — первая, «<id>@<площадка>» — остальные).
    Some(match take(core, deal, who, retry).await {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Ok(format!(
            "запись «USDT не вернулись» сделки {deal} уже решена или не существует — команда проигнорирована"
        )),
        Err(e) => Err(e),
    })
}

/// Под замком учёта: часть «не вернулись» → «идёт» («Перегнать», попытка — отдельной задачей) или → «вручную».
/// Принимает: ядро, ключ части (решение 227), кто, «Перегнать» ли. Возвращает: Ok(Some(текст)) — выполнено;
/// Ok(None) — записи нет; Err — кнопка недоступна (причина).
async fn take(core: &Core, key: &str, who: &str, retry: bool) -> Result<Option<String>, String> {
    let (deal, part) = super::usdt_return_parts::parse(key);
    let (d, amount, at, to) = {
        let mut deals = lk(&core.inner.deals);
        let Some(d) = deals.get_mut(deal) else {
            return Ok(None);
        };
        let Some(UsdtReturn::NotReturned { mut amount, at, .. }) =
            super::usdt_return_parts::get(d, part.as_ref()).cloned()
        else {
            return Ok(None);
        };
        let to = d.direction.buy_on.clone();
        if retry {
            if let Some(w) = retry_block(d, &at, &to) {
                return Err(w);
            }
            // Решение 224: у «Продано вручную» — не больше выручки продаж бота на площадке части (решение 227; выручку
            // человека бот не трогает).
            if d.usdt_return_skip.is_some() {
                amount = amount.min(super::usdt_return_deal::amount_of(core, d, &at));
            }
            // Ревью 08.11: у старых сделок (до 227) — не больше своей выручки на площадке части.
            amount = super::usdt_return_deal::own_cap(core, d, &at, amount);
            super::usdt_return_parts::put(
                d,
                UsdtReturn::Pending {
                    amount,
                    from: at.clone(),
                    to: to.clone(),
                },
            );
            d.note(format!(
                "«Перегнать» ({who}): ещё одна попытка возврата USDT {} → {}",
                at.0, to.0
            ));
        } else {
            super::usdt_return_parts::put(
                d,
                UsdtReturn::Manual {
                    amount,
                    at: at.clone(),
                    to: to.clone(),
                    by: who.to_string(),
                },
            );
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
            "Сделка {deal}: перегон USDT {} → {} запущен (сумма части {})",
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
            // Решение 201: ждать конец паузы площадки после 429 (patience_scope.rs).
            let res = super::patience_scope::with(
                &c,
                super::usdt_return_attempt::attempt(&c, &d, cap, (&at, &to), &tag),
            )
            .await;
            match res {
                Ok((sent, r)) => {
                    let t = format!(
                        "USDT вернулись («Перегнать», {who}): {sent} {} → {}, зачислено {}{}",
                        at.0,
                        to.0,
                        r.credited,
                        r.tail()
                    );
                    // Решение 236: затраты возврата — в итог сделки (usdt_return_state::set).
                    let state = r.done(sent, at, to);
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
