//! Файл: core/dirctx.rs
//! Что делает: всё, что нужно расчёту направления и не зависит от объёма: площадки и их данные о сети
//! BSC, сеть для вывода и депозита, котируемые монеты, адрес USDT, σ на B, время в пути T, k, h,
//! режим возврата USDT, минимумы вывода и минимальный депозит. Собирается один раз на кандидата.
//! Время в пути — transit.rs (решение 50); σ — по окну profit.sigma_window_min (решение 49, vol.rs).
//! Кто вызывает: core/profit.rs, позже executor.rs.
//! От чего зависит: engine.rs, registry.rs, vol.rs, settings.
//! Настройки: venue.quote_asset, profit.price_move_k (можно по монете), profit.liquidity_share_h (по B),
//! profit.sigma_window_min, usdt_return.mode (+ transit.rs).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Direction, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::engine::{Core, lk, rd};
use super::registry::Presence;

/// Неизменная часть расчёта направления.
pub struct DirCtx {
    pub dir: Direction,
    pub a: Presence,
    pub b: Presence,
    /// Название сети BSC для вывода с A и для депозита на B (у DEX — сеть биржи-напарника).
    pub net_a: String,
    pub net_b: String,
    /// Котируемые монеты A и B (первая — A).
    pub quote: Vec<String>,
    /// USDT в BSC — для возврата USDT.
    pub usdt: Asset,
    /// Включённые биржи — откуда брать цену BNB.
    pub cex: Vec<VenueId>,
    pub sigma: Decimal,
    pub transit_s: Decimal,
    pub k: Decimal,
    pub h: Decimal,
    pub return_per_trade: bool,
}

impl DirCtx {
    /// Собирает контекст. Ошибки: текст причины (монеты или площадки нет в справочнике, не задана
    /// настройка, σ ещё не измерена, нет сети BSC ни у одной стороны, нет адреса USDT).
    pub fn build(core: &Core, dir: &Direction) -> Result<DirCtx, String> {
        let s = &core.inner.deps.settings;
        let coins = rd(&core.inner.coins);
        let entry = coins
            .iter()
            .find(|c| c.asset == dir.asset)
            .ok_or("монеты нет в справочнике")?;
        let a = entry.at(&dir.buy_on).ok_or("A нет у монеты")?.clone();
        let b = entry.at(&dir.sell_on).ok_or("B нет у монеты")?.clone();
        let net_a = a.network.clone().or(b.network.clone());
        let net_b = b.network.clone().or(a.network.clone());
        let (Some(net_a), Some(net_b)) = (net_a, net_b) else {
            return Err("ни у одной стороны нет сети BSC биржи".into());
        };

        let qa = s
            .text("venue.quote_asset", at(&dir.buy_on))
            .map_err(|e| e.message)?;
        let qb = s
            .text("venue.quote_asset", at(&dir.sell_on))
            .map_err(|e| e.message)?;
        let usdt = rd(&core.inner.usdt).clone()?;
        let cex: Vec<VenueId> = rd(&core.inner.kinds)
            .iter()
            .filter(|(v, k)| *k == VenueKind::Cex && super::registry_sync::enabled(core, v))
            .map(|(v, _)| v.clone())
            .collect();
        let window = s
            .integer("profit.sigma_window_min", At::default())
            .map_err(|e| e.message)?;
        let sigma = lk(&core.inner.vol).sigma(
            &dir.sell_on,
            &dir.asset,
            Side::Sell,
            window,
            chrono::Utc::now(),
        )?;
        let coin_at = At {
            venue: None,
            coin: Some(&dir.asset.ticker),
        };
        let k = s
            .decimal("profit.price_move_k", coin_at)
            .map_err(|e| e.message)?;
        let h = s
            .decimal("profit.liquidity_share_h", at(&dir.sell_on))
            .map_err(|e| e.message)?;
        if h.is_zero() {
            return Err("profit.liquidity_share_h = 0: глубину B использовать нельзя".into());
        }
        let mode = s
            .text("usdt_return.mode", At::default())
            .map_err(|e| e.message)?;
        let transit_s = super::transit::seconds(core, dir)?;
        Ok(DirCtx {
            dir: dir.clone(),
            a,
            b,
            net_a,
            net_b,
            quote: vec![qa.clone(), qb],
            usdt: Asset {
                ticker: qa,
                bsc_contract: Some(usdt),
            },
            cex,
            sigma,
            transit_s,
            k,
            h,
            // Варианты настройки — per_trade / separate (settings/catalog/trade.rs).
            return_per_trade: mode == "per_trade",
        })
    }

    /// Минимум и максимум вывода A (из API биржи). У DEX ограничений нет.
    /// Ошибки: количество вне пределов.
    pub fn check_withdraw(&self, qty: Decimal) -> Result<(), String> {
        let Some(i) = &self.a.info else { return Ok(()) };
        if let Some(min) = i.withdraw_min
            && qty < min
        {
            return Err(format!("вывод {qty} меньше минимума {min}"));
        }
        if let Some(max) = i.withdraw_max
            && !max.is_zero()
            && qty > max
        {
            return Err(format!("вывод {qty} больше максимума {max}"));
        }
        Ok(())
    }

    /// Минимальный депозит B (решение 35). У DEX ограничений нет. Ошибки: неизвестен или больше прихода.
    pub fn check_deposit(&self, qty: Decimal) -> Result<(), String> {
        let Some(i) = &self.b.info else { return Ok(()) };
        match i.deposit_min {
            None => Err(format!(
                "{}: минимальный депозит неизвестен — сделки нет",
                self.dir.sell_on.0
            )),
            Some(min) if qty < min => {
                Err(format!("приход {qty} меньше минимального депозита {min}"))
            }
            Some(_) => Ok(()),
        }
    }
}

/// Область настройки «площадка». Принимает: площадку. Возвращает: At для чтения настроек.
fn at(v: &VenueId) -> At<'_> {
    At {
        venue: Some(&v.0),
        coin: None,
    }
}
