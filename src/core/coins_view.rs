//! Файл: core/coins_view.rs
//! Что делает: данные экрана «Монеты и направления» в формате GET /api/coins (src/api/README.md):
//! монета, площадки, налоги и их проверка, чёрный список, пулы с крючками и проверками; плюс
//! причины отсева по площадкам (presence), проблема оценки (problem) и рабочие направления;
//! пулы с USDT и с промежуточными монетами (pair), через какую промежуточную монету допущена (via, решение 52).
//! Кто вызывает: api (маршрут GET /api/coins — пишет агент интерфейса) через Core::coins_json.
//! От чего зависит: engine.rs, registry.rs, market_state.rs, blacklist.rs.
//! Настройки: нет.

use serde_json::{Value, json};

use crate::core::types::CheckState;

use super::blacklist;
use super::engine::{Core, rd};
use super::intermediate::{self, Intermediate};

impl Core {
    /// Список монет для экрана. Возвращает: JSON-массив. Ошибок нет.
    /// Налоги — с первой DEX, где у монеты есть проверка (все проверки — в поле checks).
    pub fn coins_json(&self) -> Value {
        let coins = rd(&self.inner.coins);
        let market = rd(&self.inner.market);
        let usdt = rd(&self.inner.usdt).clone().ok();
        let s = &self.inner.deps.settings;
        let nets = rd(&self.inner.networks);
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
                // Пулы монеты с USDT и (решение 52) с промежуточными монетами площадки; pair — второй токен.
                let mut pools: Vec<Value> = vec![];
                if let (Some(t), Some(u)) = (c.asset.bsc_contract, usdt) {
                    for p in &dex {
                        let mut others = vec![(u, "USDT".to_string())];
                        if let Intermediate::On(list) = intermediate::read(s, &p.venue) {
                            others
                                .extend(list.iter().map(|m| (*m, intermediate::label(&nets, *m))));
                        }
                        for (other, pair) in others {
                            for pl in market.usdt_pools(&p.venue, t, other) {
                                let check = market.pool_check(&pl.venue, &pl.reference);
                                pools.push(json!({
                                    "venue": pl.venue,
                                    "pair": pair,
                                    "family": pl.family,
                                    "kind": pl.kind,
                                    "pool": pl.reference,
                                    "fee_pips": pl.fee_pips,
                                    "hooks": pl.hooks,
                                    "check": check,
                                    "admission": blacklist::pool_admission(pl, check, s).err(),
                                    "block": pl.block,
                                    "received_at": pl.received_at,
                                }));
                            }
                        }
                    }
                }
                // Через какую промежуточную монету допущена монета на каждой DEX (решение 52).
                let via: Vec<Value> = dex
                    .iter()
                    .filter_map(|p| {
                        p.via.map(
                            |m| json!({ "venue": p.venue, "via": intermediate::label(&nets, m) }),
                        )
                    })
                    .collect();
                json!({
                    "asset": c.asset,
                    "venues": c.venues(),
                    "taxes": first.map(|k| k.taxes.clone()).unwrap_or(CheckState::NotChecked),
                    "tax_source": first.map(|k| k.source.clone()),
                    "checked_at": first.and_then(|k| k.checked_at),
                    "tax_wait": c.wait,
                    "checks": checks,
                    "block": c.block,
                    "problem": c.problem,
                    "presence": c.presence,
                    "directions": c.directions(),
                    "pools": pools,
                    "via": via,
                })
            })
            .collect();
        Value::Array(list)
    }
}
