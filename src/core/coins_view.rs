//! Файл: core/coins_view.rs
//! Что делает: данные экрана «Монеты и направления» в формате GET /api/coins (src/api/README.md):
//! монета, площадки, налоги и их проверка, чёрный список, пулы с крючками и проверками; плюс
//! причины отсева по площадкам (presence), проблема оценки (problem) и рабочие направления.
//! Кто вызывает: api (маршрут GET /api/coins — пишет агент интерфейса) через Core::coins_json.
//! От чего зависит: engine.rs, registry.rs, market_state.rs, blacklist.rs.
//! Настройки: нет.

use serde_json::{Value, json};

use crate::core::types::CheckState;

use super::blacklist;
use super::engine::{Core, rd};

impl Core {
    /// Список монет для экрана. Возвращает: JSON-массив. Ошибок нет.
    /// Налоги — с первой DEX, где у монеты есть проверка (все проверки — в поле checks).
    pub fn coins_json(&self) -> Value {
        let coins = rd(&self.inner.coins);
        let market = rd(&self.inner.market);
        let usdt = rd(&self.inner.usdt).clone().ok();
        let s = &self.inner.deps.settings;
        let list: Vec<Value> = coins
            .iter()
            .map(|c| {
                let dex: Vec<_> = c.presence.iter().filter(|p| p.info.is_none()).collect();
                let checks: Vec<_> = match c.asset.bsc_contract {
                    Some(t) => dex
                        .iter()
                        .filter_map(|p| market.coin_check(&p.venue, t))
                        .collect(),
                    None => vec![],
                };
                let first = checks.first();
                let pools: Vec<Value> = match (c.asset.bsc_contract, usdt) {
                    (Some(t), Some(u)) => dex
                        .iter()
                        .flat_map(|p| market.usdt_pools(&p.venue, t, u))
                        .map(|p| {
                            let check = market.pool_check(&p.venue, &p.reference);
                            json!({
                                "venue": p.venue,
                                "family": p.family,
                                "kind": p.kind,
                                "pool": p.reference,
                                "fee_pips": p.fee_pips,
                                "hooks": p.hooks,
                                "check": check,
                                "admission": blacklist::pool_admission(p, check, s).err(),
                                "block": p.block,
                                "received_at": p.received_at,
                            })
                        })
                        .collect(),
                    _ => vec![],
                };
                json!({
                    "asset": c.asset,
                    "venues": c.venues(),
                    "taxes": first.map(|k| k.taxes.clone()).unwrap_or(CheckState::NotChecked),
                    "tax_source": first.map(|k| k.source.clone()),
                    "checked_at": first.and_then(|k| k.checked_at),
                    "checks": checks,
                    "block": c.block,
                    "problem": c.problem,
                    "presence": c.presence,
                    "directions": c.directions(),
                    "pools": pools,
                })
            })
            .collect();
        Value::Array(list)
    }
}
