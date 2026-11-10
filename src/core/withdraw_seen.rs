//! Файл: core/withdraw_seen.rs
//! Что делает: решение 253, п. (д), дополнение (Тигран 10.11: «никаких двойных учётов и недоучётов») — факты перевода
//! монеты с биржи и поправка затрат вывода следующих сделок по ним.
//!
//! Факт — СЫРЫЕ числа последнего перевода монеты с площадки: отправлено S, зачислено C, фактическая комиссия вывода F
//! из ответа биржи в монете (у Binance — transactionFee истории выводов; None — биржа не сообщила). Производных (%
//! недостачи) не храним: они зависят от того, что модель знает В МОМЕНТ РАСЧЁТА.
//!
//! Разложение потери перевода S − C (всё в монете): S − C = комиссия + налог перевода + прочее, где
//! - комиссия F_исп = F, если биржа её сообщила, иначе комиссия справочника на S (CoinNetworkInfo::fee_for(S));
//! - налог = (S − F_исп) × t, t — налог перевода, который модель берёт СЕЙЧАС (DirCtx.transfer_tax, transfer_tax.rs);
//! - прочее = ожидалось − пришло, ожидалось = (S − F_исп) × (1 − t); доля прочего r = прочее / ожидалось (не меньше 0).
//!
//! Поправка (model) при transfer.seen_cost = да заменяет строки вывода справочника на сумму A:
//! - комиссия = комиссия справочника на A + (F − комиссия справочника на S) (не меньше 0): у фиксированной комиссии —
//!   ровно факт, у доли суммы — доля справочника плюс замеренное отличие; F не сообщена — справочник как есть;
//! - налог перевода = (A − комиссия) × t (от той же комиссии, что строка выше);
//! - TransferShortfall «недостача перевода по факту» = (A − комиссия) × (1 − t) × r.
//!
//! Почему ровно один раз: при A = S сумма трёх строк равна S − C (комиссия F + (S − F)·t + (S − F)(1 − t)·r = S − C).
//! Налог потом измерили (t вырос) — r при расчёте пересчитывается от нового t и уменьшается на то, что теперь объясняет
//! налог: двойного счёта нет. Комиссия из API = 0, а биржа удержала — удержанное попадает в F (если биржа его сообщила)
//! или в прочее (если нет): недоучёта нет. Ожидание зачисления и сверка недостачи (credit_expect.rs, credit_shortfall.rs)
//! поправку НЕ берут: факт меряется против комиссии и налога, иначе прочее следующего перевода вычло бы само себя.
//! Пришло больше ожидаемого (r < 0) — прочее 0: модель знает налог/комиссию по фактам API и пробы и не уменьшает их.
//!
//! Последний или N фактов: берётся последний перевод. Факт хранится сырым и пересчитывается против текущей модели, поэтому
//! устаревание «уже учтённого» невозможно; среднее по N смешало бы разные правила комиссии биржи (смена комиссии — разрыв,
//! а не шум) и потребовало бы ещё одной настройки; разброс прочего у одного токена — округления (доли процента от
//! недостачи). Отличие при одном аномальном переводе — до следующего перевода, видно строкой в расчёте.
//!
//! Кто вызывает: core/credit_shortfall.rs (record), core/costs_source.rs (adjust), core/relay_value.rs (adjust_at —
//! оценка «Перегона» тем же разложением), core/engine.rs (load), тесты (model).
//! От чего зависит: engine.rs, persist.rs, settings, core/types, dirctx.rs.
//! Настройки: transfer.seen_cost (вся система или по площадке-отправителю, необязательная).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, Cost, CostKind, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::dirctx::DirCtx;
use super::engine::Core;

/// Вид записи в учёте ядра (сырые факты; прежний вид withdraw_seen с долей недостачи не читается).
const KIND: &str = "withdraw_fact";

/// Сырые факты последнего перевода монеты с площадки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub asset: Asset,
    pub from: VenueId,
    /// Отправлено (сумма вывода), в монете.
    pub sent: Decimal,
    /// Зачислено получателю, в монете.
    pub credited: Decimal,
    /// Фактическая комиссия вывода в монете из ответа биржи; None — не сообщила.
    pub fee: Option<Decimal>,
}

/// Факты по (монета, площадка). Одно ядро на процесс — общая память.
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
            let m = format!("Факты переводов (решение 253) не загружены: {e}");
            core.inner
                .deps
                .log(Level::Error, "core.withdraw_seen", None, m, None)
                .await;
        }
    }
}

/// Что делает: записывает факты перевода (заменяют прежние) и сохраняет. Принимает: ядро, факты. Ошибок нет: сбой
/// базы — журнал.
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

/// Что делает: затраты вывода с биржи A с поправкой по фактам (см. шапку). Принимает: ядро, контекст, сумму вывода,
/// затраты справочника (costs_withdraw.rs withdraw_a). Возвращает: затраты — те же, если transfer.seen_cost не «да»,
/// A не биржа, фактов нет или их не к чему отнести. Ошибок нет.
pub fn adjust(core: &Core, cx: &DirCtx, amount: Decimal, dir: Vec<Cost>) -> Vec<Cost> {
    let d = &cx.dir;
    let Some(info) = &cx.a.info else {
        return dir;
    };
    let t = cx.transfer_tax.unwrap_or_default();
    adjust_at(core, &d.buy_on, &d.asset, info, t, amount, dir)
}

/// Что делает: то же для любого вывода с биржи (расчёт сделки — adjust; оценка «Перегона» — relay_value.rs): одна
/// формула на всех путях. Принимает: ядро, биржу-отправителя, монету (ядра), её сеть в справочнике, налог перевода
/// модели t (%), сумму, затраты справочника. Возвращает: затраты (как есть — transfer.seen_cost не «да» или фактов нет).
/// Ошибок нет.
pub fn adjust_at(
    core: &Core,
    from: &VenueId,
    asset: &Asset,
    info: &crate::core::types::CoinNetworkInfo,
    t: Decimal,
    amount: Decimal,
    dir: Vec<Cost>,
) -> Vec<Cost> {
    let at = At {
        venue: Some(&from.0),
        coin: None,
    };
    let on = core.inner.deps.settings.flag("transfer.seen_cost", at);
    if !on.unwrap_or(false) {
        return dir;
    }
    let Some(s) = get(asset, from) else {
        return dir;
    };
    model(&s, info.fee_for(s.sent), t, amount, dir)
}

/// Что делает: разложение факта и затраты на сумму A (чистая функция, формулы — шапка). Принимает: факты, комиссию
/// справочника на S (None — неизвестна), налог перевода модели t (%), сумму A, затраты справочника на A.
/// Возвращает: затраты на A (строки WithdrawFee и TransferTax справочника заменены; остальные — как есть; прочее —
/// строка TransferShortfall). Ошибок нет: комиссию на S не из чего взять или ожидание ≤ 0 — затраты как есть.
pub fn model(
    s: &Seen,
    fee_dir_s: Option<Decimal>,
    t: Decimal,
    amount: Decimal,
    dir: Vec<Cost>,
) -> Vec<Cost> {
    let Some(fee_s) = s.fee.or(fee_dir_s) else {
        return dir;
    };
    let keep = Decimal::ONE - t / Decimal::ONE_HUNDRED;
    let expected_s = (s.sent - fee_s) * keep;
    if expected_s <= Decimal::ZERO {
        return dir;
    }
    let r = ((expected_s - s.credited) / expected_s).max(Decimal::ZERO);
    let fee_dir_a: Decimal = dir
        .iter()
        .filter(|c| c.kind == CostKind::WithdrawFee)
        .map(|c| c.amount)
        .sum();
    let fee_a = match (s.fee, fee_dir_s) {
        (Some(f), Some(dir_s)) => (fee_dir_a + f - dir_s).max(Decimal::ZERO),
        _ => fee_dir_a,
    };
    let ticker = s.asset.ticker.clone();
    let base = (amount - fee_a).max(Decimal::ZERO);
    let mut out: Vec<Cost> = dir
        .into_iter()
        .filter(|c| !matches!(c.kind, CostKind::WithdrawFee | CostKind::TransferTax))
        .collect();
    out.push(Cost::new(
        CostKind::WithdrawFee,
        fee_a,
        ticker.clone(),
        false,
    ));
    out.push(Cost::new(
        CostKind::TransferTax,
        base * t / Decimal::ONE_HUNDRED,
        ticker.clone(),
        false,
    ));
    let other = base * keep * r;
    if other > Decimal::ZERO {
        out.push(Cost::new(CostKind::TransferShortfall, other, ticker, false));
    }
    out
}
