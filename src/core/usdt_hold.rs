//! Файл: core/usdt_hold.rs
//! Что делает: решение 405 (t191, ответ Тиграна 18.11: «резервировать») — резерв USDT, ждущих общего возврата группы
//! докупки. Закрытая сделка группы оставляет выручку на площадке продажи (Deal.group.wait: площадка, сумма — ставит
//! coin_group.rs hold_parts) до общего возврата группы; пока они ждут, покупка любой сделки (любой монеты, любого
//! направления) на этой площадке их не тратит: предел покупки = свободно − резерв (limits.rs buy_free → buy_cap и
//! предпроверка precheck_balance.rs). Резерва больше свободного — свободно 0 и причина «ждут общего возврата».
//! Кошелёк: у DEX — только сделки того же кошелька (Deal.dex_account, решение 132); у биржи — все сделки площадки.
//! Резерв снимается сам: общий возврат начат — wait снят (coin_group_flush.rs); перезапуск — wait снят, «не вернулись»
//! (ledger_load.rs). Чего здесь НЕТ: части возврата, уже начатой перегоном (Pending / Retrying, оставшиеся куски), —
//! они не резервируются (как у одиночного возврата до решения 394; docs/TASKS.md, решение 405).
//! Быстрый путь: нет идущих сделок групп (inner.group_live = 0) и нет пулов групп (inner.groups пуст) — отложенных
//! частей не бывает (wait ставится, пока запись сделки ещё в идущих; общий возврат и запуск их снимают) — 0 без
//! прохода по учёту.
//! Кто вызывает: limits.rs (buy_free, buy_cap), precheck_balance.rs (read).
//! От чего зависит: engine.rs (deals, groups, group_live), trade_ops.rs (вид площадки), deal.rs, deal_group.rs.
//! Настройки: нет (базовое поведение).

use std::sync::atomic::Ordering;

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::contract::VenueKind;

use super::engine::{Core, lk};

/// Резерв площадки: сумма и сделки, чья выручка ждёт общего возврата.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Held {
    /// USDT, ждущие общего возврата групп докупки.
    pub amount: Decimal,
    /// Сделки (ID) и их группы — для текста причины.
    pub deals: Vec<(String, String)>,
}

/// Что делает: отложенные части общего возврата на площадке `venue` (у DEX — кошелёк `acc`). Принимает: ядро,
/// площадку, счёт DEX покупки (None — биржа или счёт неизвестен). Возвращает: резерв (пусто — нет). Ошибок нет: вид
/// площадки неизвестен — сверка по кошельку как у DEX (резерв не теряется).
pub fn held(core: &Core, venue: &VenueId, acc: Option<&DexAccountId>) -> Held {
    if core.inner.group_live.load(Ordering::Relaxed) == 0 && lk(&core.inner.groups).is_empty() {
        return Held::default();
    }
    let cex = super::trade_ops::kind(core, venue) == Ok(VenueKind::Cex);
    let deals = lk(&core.inner.deals);
    let mut out = Held::default();
    for d in deals.values() {
        let Some(g) = d.group.as_ref() else {
            continue;
        };
        let Some((from, amount)) = g.wait.as_ref() else {
            continue;
        };
        // У DEX — только тот же кошелёк; у биржи — любой.
        let same = cex || d.dex_account.as_ref() == acc;
        if from != venue || !same {
            continue;
        }
        out.amount += *amount;
        out.deals.push((d.id.clone(), g.id.clone()));
    }
    out.deals.sort();
    out
}

/// Что делает: свободно для покупки = прочитанное свободно − резерв (не меньше 0). Принимает: свободно, резерв.
/// Ошибок нет.
pub fn spendable(free: Decimal, h: &Held) -> Decimal {
    (free - h.amount).max(Decimal::ZERO)
}

/// Что делает: хвост текста причины «на X свободно 0 …» при резерве: сколько USDT и чьи. Принимает: резерв, тикер
/// котируемой монеты. Возвращает: пустую строку — резерва нет. Ошибок нет.
pub fn suffix(h: &Held, quote: &str) -> String {
    if h.amount <= Decimal::ZERO {
        return String::new();
    }
    let who: Vec<String> = h
        .deals
        .iter()
        .map(|(d, g)| format!("{d} (группа {g})"))
        .collect();
    format!(
        " — ещё {} {quote} ждут общего возврата групп докупки: {}; их не трогаем (решение 405)",
        h.amount,
        who.join(", ")
    )
}
