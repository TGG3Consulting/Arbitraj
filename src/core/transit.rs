//! Файл: core/transit.rs
//! Что делает: время в пути T для запаса r (решения 50, 51). Настройки — свои у каждого направления
//! перевода (область — площадка, с которой переводим). Переключатель profit.transit_mode:
//! «вручную» (manual) — всегда profit.transit_manual_s; «вручную + авто» (manual_auto) — время из
//! настройки, пока по монете в этом направлении меньше profit.transit_auto_after переводов, затем —
//! среднее по последним profit.transit_auto_after реальным переводам. Расчёт по подтверждениям блоков
//! не используется. История переводов хранится в учёте ядра (SQLite) и переживает перезапуск.
//! Кто вызывает: core/dirctx.rs (T), core/transfer_watch.rs (запись перевода), core/engine.rs (загрузка).
//! От чего зависит: engine.rs, persist.rs, settings.
//! Настройки: profit.transit_mode, profit.transit_manual_s, profit.transit_auto_after (по площадке-источнику).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, Direction, VenueId};
use crate::settings::scope::Scope;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

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

/// id записи в учёте: адрес контракта (или тикер), откуда, куда.
fn id(a: &Asset, from: &VenueId, to: &VenueId) -> String {
    let coin = a
        .bsc_contract
        .map(|x| x.to_string())
        .unwrap_or(a.ticker.clone());
    format!("{coin}:{}:{}", from.0, to.0)
}

/// Загружает историю при запуске. Ошибка базы — в журнал (тогда «авто» начнёт набирать заново).
pub async fn load(core: &Core) {
    match super::persist::load::<Record>(&core.inner.deps.db, KIND).await {
        Ok(list) => {
            let mut t = lk(&core.inner.transits);
            for r in list {
                t.insert((r.asset, r.from, r.to), r.secs);
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

/// Записывает фактическое время перевода и сохраняет историю. Ошибка базы — в журнал.
pub async fn record(core: &Core, asset: &Asset, from: &VenueId, to: &VenueId, secs: Decimal) {
    let rec = {
        let mut t = lk(&core.inner.transits);
        let v = t
            .entry((asset.clone(), from.clone(), to.clone()))
            .or_default();
        v.push(secs);
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

/// Время в пути направления, секунды. Все три настройки — ровно в области площадки-источника перевода
/// (A, откуда выводим; решение 51): bingx — BingX → кошелёк, bsc_dex — кошелёк → биржа.
/// Статистика — по монете и направлению (откуда, куда).
/// Ошибки: текст «не задана настройка … [venue:…]» или неизвестный режим.
pub fn seconds(core: &Core, d: &Direction) -> Result<Decimal, String> {
    let s = &core.inner.deps.settings;
    let from = Scope::Venue(d.buy_on.0.clone());
    let mode = s
        .text_in("profit.transit_mode", &from)
        .map_err(|e| e.message)?;
    let manual = s
        .integer_in("profit.transit_manual_s", &from)
        .map(Decimal::from)
        .map_err(|e| e.message)?;
    // Варианты настройки — manual / manual_auto (settings/catalog/base.rs).
    match mode.as_str() {
        "manual" => Ok(manual),
        "manual_auto" => {
            let n = s
                .integer_in("profit.transit_auto_after", &from)
                .map_err(|e| e.message)? as usize;
            let t = lk(&core.inner.transits);
            let hist = t.get(&(d.asset.clone(), d.buy_on.clone(), d.sell_on.clone()));
            match hist {
                Some(h) if n > 0 && h.len() >= n => {
                    let last = &h[h.len() - n..];
                    Ok(last.iter().copied().sum::<Decimal>() / Decimal::from(n))
                }
                _ => Ok(manual),
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
