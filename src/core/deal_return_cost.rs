//! Файл: core/deal_return_cost.rs
//! Что делает: решение 236 (Тигран 08–09.11: «везде, всегда — точная, честная сумма с учётом ВСЕХ затрат, и в плане, и в
//! фиксации прибыли/убытка») — затраты возврата выручки USDT на площадку покупки входят в итог сделки (result_usd).
//! Было (факт кода): итог = выручка − вложено − затраты (ledger::finish), возврат шёл после закрытия, его затраты
//! (комиссия вывода USDT, газ кошелька DEX) в итог не входили; план их считал (costs.rs, строки «возврат USDT»).
//! Затраты части «вернулись» = ушло с площадки после округления до точности вывода (sent) − зачислено + затраты вывода
//! не в USDT в USDT (газ BNB кошелька — fee_usd). Комиссия вывода в USDT уже в разнице sent − credited (так же, как
//! комиссия вывода монеты — в «отправлено − дошло», fills::transfer; на данных бота разница = комиссии справочника).
//! Остаток округления (amount − sent) лежит на площадке-отправителе — деньги не потеряны, не затрата.
//! Запись до решения 236 (sent нет): затраты = amount − credited (другого факта в записи нет; включает остаток
//! округления, газа нет). «Не вернулись» — затрат возврата нет: в итог не входят. «Перегнано вручную» (решение 236,
//! дополнение п. 2, техлид 09.11): человек отвечает, сколько зачислено, — затраты = сумма части − зачислено, как у
//! «вернулись»; не ответил («-») — неизвестны, в итог не входят (пометка).
//! Итог меняется, когда часть становится «вернулись» (usdt_return_state::set): итог дня (daily::settle с −разницей),
//! статистика монеты (gas_stats), журнал core.usdt_return; старые записи — при загрузке, в памяти (ledger_load.rs).
//! Кто вызывает: usdt_return_state.rs (set), ledger.rs (finish), ledger_load.rs, usdt_return.rs (relay_try — Relayed),
//! deal_text.rs, manual_outcome.rs.
//! От чего зависит: deal.rs, usdt_return_state.rs, usdt_return_parts.rs, daily.rs, daily_commit.rs, gas_stats.rs,
//! fills.rs (перевод затраты в USDT), deps.rs (журнал), deal_book.rs (начисление поправки в учёт сделки, решение 257).
//! Настройки: нет (котируемая монета площадки venue.quote_asset — у вызывающего).

use rust_decimal::Decimal;

use crate::core::types::{Cost, VenueId};
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::usdt_return_state::UsdtReturn;

/// Итог одного перегона USDT (usdt_return::relay_try).
#[derive(Debug, Clone, PartialEq)]
pub struct Relayed {
    /// Ушло с площадки: запрошенная сумма вниз до точности и шага вывода (transfers::floored).
    pub sent: Decimal,
    /// Зачислено на получателе.
    pub credited: Decimal,
    /// Затраты вывода не в котируемой монете (газ BNB кошелька DEX), USDT. None — площадка не сообщила или нет курса.
    pub fee_usd: Option<Decimal>,
    /// Почему затрата вывода не пересчитана в USDT (нет курса) — для хронологии. None — пересчитана или её нет.
    pub fee_note: Option<String>,
}

impl Relayed {
    /// Что делает: состояние части «вернулись» по итогу перегона. Принимает: запрошенную сумму, откуда, куда.
    /// Возвращает: UsdtReturn::Done с sent и fee_usd. Ошибок нет.
    pub fn done(&self, amount: Decimal, from: VenueId, to: VenueId) -> UsdtReturn {
        UsdtReturn::Done {
            amount,
            credited: self.credited,
            from,
            to,
            sent: Some(self.sent),
            fee_usd: self.fee_usd,
        }
    }

    /// Что делает: хвост строки хронологии — затраты возврата и пересчёт газа. Возвращает: строку («» — нечего
    /// добавить). Ошибок нет.
    pub fn tail(&self) -> String {
        let cost = self.sent - self.credited + self.fee_usd.unwrap_or_default();
        let mut t = format!(
            "; затраты возврата {} USDT — в итоге сделки",
            cost.round_dp(6)
        );
        if let Some(n) = &self.fee_note {
            t.push_str(&format!("; {n}"));
        }
        t
    }
}

/// Что делает: затраты одной части возврата. Принимает: часть. Возвращает: Some — «вернулись» (sent или amount −
/// credited + fee_usd) и «перегнано вручную» с ответом человека (решение 236, дополнение п. 2: amount − credited —
/// ушло с площадки сумма части, зачислено — ответ); None — другие состояния (идёт, ждёт повтора, не вернулись,
/// перегнано вручную без ответа). Ошибок нет.
pub fn part_cost(p: &UsdtReturn) -> Option<Decimal> {
    match p {
        UsdtReturn::Manual {
            amount,
            credited: Some(c),
            ..
        } => Some(*amount - *c),
        UsdtReturn::Done {
            amount,
            credited,
            sent,
            fee_usd,
            ..
        } => Some(sent.unwrap_or(*amount) - *credited + fee_usd.unwrap_or_default()),
        _ => None,
    }
}

/// Что делает: известные затраты возврата сделки — сумма по частям «вернулись». Принимает: сделку. Ошибок нет.
pub fn known(d: &Deal) -> Decimal {
    super::usdt_return_parts::all(d)
        .into_iter()
        .filter_map(part_cost)
        .sum()
}

/// Что делает: возврат ещё идёт — есть часть «идёт» или «ждёт повтора». Принимает: сделку. Ошибок нет.
pub fn in_progress(d: &Deal) -> bool {
    super::usdt_return_parts::all(d)
        .iter()
        .any(|p| matches!(p, UsdtReturn::Pending { .. } | UsdtReturn::Retrying { .. }))
}

/// Что делает: части без известной стоимости возврата («не вернулись», «перегнано вручную» без ответа человека —
/// решение 236, дополнение п. 2). Ошибок нет.
pub fn unknown_parts(d: &Deal) -> usize {
    super::usdt_return_parts::all(d)
        .iter()
        .filter(|p| {
            matches!(
                p,
                UsdtReturn::NotReturned { .. } | UsdtReturn::Manual { credited: None, .. }
            )
        })
        .count()
}

/// Что делает: вносит известные затраты возврата в итог закрытой сделки (идемпотентно: вычитается только разница с
/// уже учтённым return_cost_usd). Принимает: сделку. Возвращает: на сколько выросли затраты (итог уменьшен на столько;
/// 0 — без изменений, итога нет или частей нет). Ошибок нет.
pub fn apply(d: &mut Deal) -> Decimal {
    if d.result_usd.is_none() || super::usdt_return_parts::all(d).is_empty() {
        return Decimal::ZERO;
    }
    let k = known(d);
    let delta = k - d.return_cost_usd.unwrap_or_default();
    if let Some(r) = d.result_usd.as_mut() {
        *r -= delta;
    }
    d.return_cost_usd = Some(k);
    delta
}

/// Что делает: после роста затрат возврата закрытой сделки — итог дня (daily::settle с −delta: сделка уже закрыта,
/// оценки нет — это поправка факта), статистика монеты, журнал. Принимает: ядро, сделку (итог уже исправлен), рост
/// затрат. Ошибок нет.
pub async fn after(core: &Core, d: &Deal, delta: Decimal) {
    if delta.is_zero() {
        return;
    }
    let mut x = d.clone();
    let before = x.day_book.len();
    let trip = super::daily::settle(core, &mut x, -delta).await;
    // Решение 257: начисление поправки в итог дня — в учёт сделки (копия x не сохраняется).
    super::deal_book::keep_day(core, &x, before).await;
    super::gas_stats::record_result(core, &d.direction.asset, -delta).await;
    let text = format!(
        "Сделка {}: затраты возврата USDT {} — вычтены из итога сделки, итог {} USDT",
        d.id,
        delta.round_dp(6),
        d.result_usd.unwrap_or_default().round_dp(4)
    );
    core.inner
        .deps
        .log(Level::Info, "core.usdt_return", Some(&d.at.0), text, None)
        .await;
    super::daily_commit::announce(core, &trip).await;
}

/// Что делает: затрата вывода USDT не в котируемой монете → USDT (газ BNB кошелька — по живой цене, fills::usdt).
/// Принимает: ядро, площадку-отправителя, котируемую монету, затрату вывода (None — площадка не сообщила).
/// Возвращает: Some(USDT); None — затраты нет или она в котируемой монете (уже в sent − credited).
/// Ошибки: текст — нет живой цены валюты затраты.
pub async fn fee_usd(
    core: &Core,
    venue: &VenueId,
    quote: &str,
    fee: Option<&Cost>,
) -> Result<Option<Decimal>, String> {
    let Some(c) = fee.filter(|c| !c.amount.is_zero() && c.asset != quote) else {
        return Ok(None);
    };
    super::fills::usdt(core, venue, quote, Decimal::ONE, c)
        .await
        .map(Some)
}
