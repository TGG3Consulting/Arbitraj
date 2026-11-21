//! Файл: core/transit.rs
//! Что делает: время в пути T для запаса r (решения 50, 51). Настройки — свои у каждого направления
//! перевода (область — площадка, с которой переводим). Переключатель profit.transit_mode:
//! «вручную» (manual) — всегда profit.transit_manual_s; «вручную + авто» (manual_auto) — время из
//! настройки, пока по монете в этом направлении меньше profit.transit_auto_after переводов, затем —
//! среднее по последним profit.transit_auto_after реальным переводам. Расчёт по подтверждениям блоков
//! не используется. История переводов хранится в учёте ядра (SQLite) и переживает перезапуск.
//! Решение 464 (t238): у монеты переводов меньше profit.transit_auto_after, а у пары площадок (все монеты) не меньше —
//! среднее записанных переводов пары (transit_source.rs; решение 487 — в той же сети перевода), иначе — «вручную»;
//! источник — seconds_from.
//! Кто вызывает: core/dirctx.rs (T), core/transit_source.rs (seconds_from), core/transfer_watch.rs (запись перевода), core/engine.rs (загрузка).
//! От чего зависит: engine.rs, persist.rs, settings.
//! Решение 118 (Я-4): «вручную» = отправка с источника (profit.transit_manual_s[A]) + зачисление на получателе
//! (profit.transit_credit_s[B], не задано — 0); история пары — не длиннее profit.transit_auto_after.
//! Настройки: profit.transit_mode, profit.transit_manual_s, profit.transit_auto_after (по площадке-источнику),
//! profit.transit_credit_s (по площадке-получателю, необязательная).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, Direction, VenueId};
use crate::settings::scope::Scope;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::transit_source::Source;

/// Вид записи в учёте ядра.
const KIND: &str = "transit";

/// История переводов одной монеты в одном направлении.
#[derive(Serialize, Deserialize)]
struct Record {
    asset: Asset,
    from: VenueId,
    to: VenueId,
    /// Длительности переводов по порядку, секунды.
    secs: Vec<Decimal>,
}

/// id записи в учёте: адрес контракта (или тикер), откуда, куда; у монеты другой сети (Asset.chain = Some) — ещё
/// «@сеть» (coin_key::rid). Решение 487 (CI t256): без сети запись монеты Arbitrum с тем же адресом / тикером
/// затирала в базе запись BSC (ключ памяти — Asset с сетью, решение 416), и после перезапуска история BSC пропадала.
/// Сеть по умолчанию (chain = None) — id прежний: старые записи не переносятся. Ошибок нет.
fn id(a: &Asset, from: &VenueId, to: &VenueId) -> String {
    format!("{}:{}:{}", super::coin_key::rid(a), from.0, to.0)
}

/// Загружает историю при запуске. Ошибка базы — в журнал (тогда «авто» начнёт набирать заново).
pub async fn load(core: &Core) {
    match super::persist::load::<Record>(&core.inner.deps.db, KIND).await {
        Ok(list) => {
            let mut t = lk(&core.inner.transits);
            for r in list {
                // Ревью t238: готовая сумма пары площадок (transit_source.rs) — вместе с историей.
                // Решение 487: сеть перевода — Asset.chain записи (старая запись без chain — сеть по умолчанию).
                let after = sum_len(Some(r.secs.as_slice()));
                let chain = r.asset.chain.clone();
                let old = t.insert((r.asset, r.from.clone(), r.to.clone()), r.secs);
                let before = sum_len(old.as_deref());
                let key = (chain.as_ref(), &r.from, &r.to);
                super::transit_source::pair_update(core, key, before, after);
            }
        }
        Err(e) => {
            let m = format!("История времени в пути не загружена: {e}");
            core.inner
                .deps
                .log(Level::Error, "core.transit", None, m, None)
                .await;
        }
    }
}

/// Сумма и длина ряда (None — пусто). Ошибок нет.
fn sum_len(v: Option<&[Decimal]>) -> (Decimal, usize) {
    v.map_or((Decimal::ZERO, 0), |x| (x.iter().copied().sum(), x.len()))
}

/// Записывает фактическое время перевода и сохраняет историю. Ошибка базы — в журнал.
pub async fn record(core: &Core, asset: &Asset, from: &VenueId, to: &VenueId, secs: Decimal) {
    let rec = {
        let mut t = lk(&core.inner.transits);
        let v = t
            .entry((asset.clone(), from.clone(), to.clone()))
            .or_default();
        // Ревью t238: ряд до правки — для готовой суммы пары площадок (transit_source.rs).
        let before = sum_len(Some(v.as_slice()));
        v.push(secs);
        // Решение 118 (Я-4): ряд — не длиннее profit.transit_auto_after пары (больше «авто» не берёт); не задана —
        // ряд не обрезается.
        let keep = core
            .inner
            .deps
            .settings
            .integer_in("profit.transit_auto_after", &Scope::Venue(from.0.clone()));
        if let Ok(n) = keep.map(|n| n as usize)
            && n > 0
            && v.len() > n
        {
            v.drain(..v.len() - n);
        }
        let after = sum_len(Some(v.as_slice()));
        let key = (asset.chain.as_ref(), from, to);
        super::transit_source::pair_update(core, key, before, after);
        Record {
            asset: asset.clone(),
            from: from.clone(),
            to: to.clone(),
            secs: v.clone(),
        }
    };
    let rid = id(asset, from, to);
    if let Err(e) = super::persist::save(&core.inner.deps.db, KIND, &rid, &rec).await {
        let m = format!("Время перевода {rid} не сохранено: {e}");
        core.inner
            .deps
            .log(Level::Error, "core.transit", None, m, None)
            .await;
    }
}

/// Время в пути направления, секунды. Режим, «отправка» и порог «авто» — ровно в области площадки-источника перевода
/// (A, откуда выводим; решение 51); «вручную» = profit.transit_manual_s[A] + profit.transit_credit_s[B] (зачисление
/// у получателя, решение 118; не задано — 0).
/// Статистика — по монете и направлению (откуда, куда); решение 464 — у монеты мало, у пары площадок достаточно —
/// статистика пары (transit_source.rs).
/// Ошибки: текст «не задана настройка … [venue:…]» или неизвестный режим.
pub fn seconds(core: &Core, d: &Direction) -> Result<Decimal, String> {
    seconds_from(core, d).map(|(s, _)| s)
}

/// Что делает: как seconds, вместе с источником числа (решение 464: настройки, статистика монеты или пары площадок).
/// Принимает: ядро, направление. Возвращает: (секунды, источник). Ошибки: как у seconds.
pub fn seconds_from(core: &Core, d: &Direction) -> Result<(Decimal, Source), String> {
    let s = &core.inner.deps.settings;
    let from = Scope::Venue(d.buy_on.0.clone());
    let mode = s
        .text_in("profit.transit_mode", &from)
        .map_err(|e| e.message)?;
    // Решение 118 (Я-4): «вручную» = отправка у источника + зачисление у получателя (не задано — 0, как раньше).
    let send = s
        .integer_in("profit.transit_manual_s", &from)
        .map(Decimal::from)
        .map_err(|e| e.message)?;
    let credit = s
        .integer_in(
            "profit.transit_credit_s",
            &Scope::Venue(d.sell_on.0.clone()),
        )
        .map(Decimal::from)
        .unwrap_or_default();
    let manual = (send + credit, Source::Manual);
    // Варианты настройки — manual / manual_auto (settings/catalog/base.rs).
    match mode.as_str() {
        "manual" => Ok((manual.0, Source::ManualMode)),
        "manual_auto" => {
            let n = s
                .integer_in("profit.transit_auto_after", &from)
                .map_err(|e| e.message)? as usize;
            let coin = {
                let t = lk(&core.inner.transits);
                match t.get(&(d.asset.clone(), d.buy_on.clone(), d.sell_on.clone())) {
                    Some(h) if n > 0 && h.len() >= n => {
                        let last = &h[h.len() - n..];
                        Some(last.iter().copied().sum::<Decimal>() / Decimal::from(n))
                    }
                    _ => None,
                }
            };
            if let Some(avg) = coin {
                return Ok((avg, Source::Coin(n)));
            }
            // Решение 464: своих переводов монеты мало — статистика пары площадок по всем монетам, если её хватает.
            match super::transit_source::pair_mean(core, d, n) {
                Some((avg, count, coins)) => Ok((avg, Source::Pair(count, coins))),
                None => Ok(manual),
            }
        }
        other => Err(format!(
            "неизвестный режим profit.transit_mode = {other} [{}]",
            from.to_key()
        )),
    }
}

impl Core {
    /// История времени в пути для интерфейса: монета, откуда, куда, число переводов, среднее.
    pub fn transits(&self) -> serde_json::Value {
        let t = lk(&self.inner.transits);
        let rows: Vec<_> = t
            .iter()
            .map(|((a, f, to), v)| {
                let avg = (!v.is_empty()).then(|| v.iter().copied().sum::<Decimal>() / Decimal::from(v.len()));
                serde_json::json!({ "asset": a, "from": f, "to": to, "count": v.len(), "avg_s": avg })
            })
            .collect();
        serde_json::Value::Array(rows)
    }
}
