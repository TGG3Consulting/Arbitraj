//! Файл: core/withdraw_seen.rs
//! Что делает: решение 251, п. (д) — факты переводов с площадки по монете для затрат следующих сделок:
//! - фактическая комиссия вывода (Transfer.fee из ответа площадки — у Binance transactionFee истории выводов, в монете);
//! - недостача зачисления сверх фактической комиссии и известного налога перевода, % (credit_shortfall.rs).
//!
//! Запись — после каждого зачисления перевода (credit_shortfall::check), хранится в учёте ядра (SQLite) и переживает
//! перезапуск; последний перевод заменяет прежний. Поправка расчёта (adjust — источник Live, costs_source.rs) при
//! transfer.seen_cost = да площадки-отправителя: комиссия вывода — не меньше фактической (строка WithdrawFee «сверх
//! справочника»), к налогу перевода — замеренная недостача сверх него (строка TransferTax). Не задана или нет —
//! расчёт как раньше (только справочник сетей и известный налог), факты всё равно записываются.
//! Кто вызывает: core/credit_shortfall.rs (record), core/costs_source.rs (adjust), core/engine.rs (load).
//! От чего зависит: engine.rs, persist.rs, settings, core/types.
//! Настройки: transfer.seen_cost (вся система или по площадке-отправителю, необязательная).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, Cost, CostKind, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;

/// Вид записи в учёте ядра.
const KIND: &str = "withdraw_seen";

/// Факты последнего перевода монеты с площадки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub asset: Asset,
    pub from: VenueId,
    /// Фактическая комиссия вывода в монете; None — площадка не сообщила.
    pub fee: Option<Decimal>,
    /// Недостача зачисления сверх комиссии и известного налога, % от ожидаемого (0 — дошло не меньше ожидаемого).
    pub shortfall_pct: Decimal,
}

/// Факты по (монета, площадка). Одно ядро на процесс — общая память (как кэши адаптеров).
static SEEN: LazyLock<Mutex<HashMap<(Asset, VenueId), Seen>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// id записи в учёте: адрес контракта (или тикер) и площадка.
fn id(a: &Asset, from: &VenueId) -> String {
    let coin = a
        .bsc_contract
        .map(|x| x.to_string())
        .unwrap_or(a.ticker.clone());
    format!("{coin}:{}", from.0)
}

/// Что делает: загружает факты при запуске. Ошибка базы — в журнал (поправки начнутся с новых переводов). Ошибок нет.
pub async fn load(core: &Core) {
    match super::persist::load::<Seen>(&core.inner.deps.db, KIND).await {
        Ok(list) => {
            let mut m = SEEN.lock().unwrap_or_else(|p| p.into_inner());
            for s in list {
                m.insert((s.asset.clone(), s.from.clone()), s);
            }
        }
        Err(e) => {
            let m = format!("Факты переводов (решение 251) не загружены: {e}");
            core.inner
                .deps
                .log(Level::Error, "core.withdraw_seen", None, m, None)
                .await;
        }
    }
}

/// Что делает: записывает факты перевода и сохраняет. Принимает: ядро, факты. Ошибок нет: сбой базы — журнал.
pub async fn record(core: &Core, s: Seen) {
    let rid = id(&s.asset, &s.from);
    SEEN.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert((s.asset.clone(), s.from.clone()), s.clone());
    if let Err(e) = super::persist::save(&core.inner.deps.db, KIND, &rid, &s).await {
        let m = format!("Факты перевода {rid} не сохранены: {e}");
        core.inner
            .deps
            .log(Level::Error, "core.withdraw_seen", None, m, None)
            .await;
    }
}

/// Факты по монете и площадке. None — переводов не было (или не загружены). Ошибок нет.
pub fn get(asset: &Asset, from: &VenueId) -> Option<Seen> {
    SEEN.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(asset.clone(), from.clone()))
        .cloned()
}

/// Что делает: поправка затрат вывода биржи по фактам (см. шапку). Принимает: ядро, монету (ядра), биржу-отправителя,
/// сумму вывода, затраты справочника (комиссия WithdrawFee и налог TransferTax в монете). Возвращает: строки сверх
/// справочника (пусто — поправки нет: настройка не задана или нет, фактов нет, факт не больше справочника). Ошибок нет.
pub fn adjust(
    core: &Core,
    asset: &Asset,
    from: &VenueId,
    amount: Decimal,
    dir: &[Cost],
) -> Vec<Cost> {
    let at = At {
        venue: Some(&from.0),
        coin: None,
    };
    if !core
        .inner
        .deps
        .settings
        .flag("transfer.seen_cost", at)
        .unwrap_or(false)
    {
        return vec![];
    }
    let Some(s) = get(asset, from) else {
        return vec![];
    };
    extra(&s, amount, dir)
}

/// Что делает: строки сверх справочника (чистая функция, тест — tests/binance_withdraw_ready.rs). Принимает: факты,
/// сумму вывода, затраты справочника. Возвращает: WithdrawFee (факт − справочник, если больше), TransferTax ((сумма −
/// комиссия − известный налог) × замеренная недостача %, если она больше нуля). Ошибок нет.
pub fn extra(s: &Seen, amount: Decimal, dir: &[Cost]) -> Vec<Cost> {
    let sum =
        |k: CostKind| -> Decimal { dir.iter().filter(|c| c.kind == k).map(|c| c.amount).sum() };
    let (fee_dir, tax_dir) = (sum(CostKind::WithdrawFee), sum(CostKind::TransferTax));
    let ticker = s.asset.ticker.clone();
    let mut out = vec![];
    let fee = match s.fee {
        Some(f) if f > fee_dir => {
            out.push(Cost::new(
                CostKind::WithdrawFee,
                f - fee_dir,
                ticker.clone(),
                false,
            ));
            f
        }
        _ => fee_dir,
    };
    // Недостача замерена сверх известного налога — от того, что дошло бы после комиссии и налога.
    let base = (amount - fee - tax_dir).max(Decimal::ZERO);
    let seen = base * s.shortfall_pct / Decimal::ONE_HUNDRED;
    if seen > Decimal::ZERO {
        out.push(Cost::new(CostKind::TransferTax, seen, ticker, false));
    }
    out
}
