//! Файл: core/dirctx.rs
//! Что делает: всё, что нужно расчёту направления и не зависит от объёма: площадки и их данные о сети
//! BSC, сеть для вывода и депозита, котируемые монеты, адрес USDT, σ на B, время в пути T, k, h,
//! режим возврата USDT, минимумы вывода и минимальный депозит, правила пар бирж (шаг количества, минимальная и
//! максимальная сумма ордера — решение 66). Собирается один раз на кандидата.
//! Время в пути — transit.rs (решение 50); σ — по окну profit.sigma_window_min (решение 49, vol.rs).
//! Кто вызывает: core/profit.rs, позже executor.rs.
//! От чего зависит: engine.rs, registry.rs, vol.rs, settings.
//! Правила пар и свежесть справочника сетей — dirctx_venue.rs (venue.networks_stale_s).
//! Настройки: venue.quote_asset, profit.price_move_k (можно по монете), profit.liquidity_share_h (по B),
//! profit.sigma_window_min, usdt_return.mode (+ transit.rs).

use rust_decimal::Decimal;

use crate::core::types::{Asset, CoinNetworkInfo, Direction, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::events::SpotPairState;
use crate::settings::read::At;

use super::dirctx_venue as venue;
use super::engine::{Core, lk, rd};
use super::registry::Presence;
use super::size::Fail;

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
    /// Правила спот-пары биржи на A и на B (решение 66); у DEX — None.
    pub pair_a: Option<SpotPairState>,
    pub pair_b: Option<SpotPairState>,
    /// Сеть BSC котируемой монеты B у биржи B (комиссия вывода — для возврата USDT без запроса к бирже, решение 66);
    /// у DEX — None.
    pub usdt_b: Option<CoinNetworkInfo>,
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
        let pair_a = venue::pair_rules(core, &dir.buy_on, &dir.asset.ticker)?;
        let pair_b = venue::pair_rules(core, &dir.sell_on, &dir.asset.ticker)?;
        // Решение 66: комиссии вывода и возврата — из последнего справочника сетей ядра (не из копии в справочнике
        // монет); справочник старше venue.networks_stale_s — сделки нет.
        let latest = |p: &Presence, v: &VenueId, t: &str, n: &str| -> Result<Option<_>, String> {
            match p.info {
                None => Ok(None),
                Some(_) => venue::network(core, v, t, n)
                    .ok_or(format!("{}: сети {n} у {t} нет в справочнике сетей", v.0))
                    .and_then(|i| venue::fresh(core, &i).map(|_| Some(i))),
            }
        };
        let ticker = &dir.asset.ticker;
        let mut a = a;
        a.info = latest(&a, &dir.buy_on, ticker, &net_a)?;
        let mut b = b;
        b.info = latest(&b, &dir.sell_on, ticker, &net_b)?;
        let usdt_b = if cx_return(&mode) {
            latest(&b, &dir.sell_on, &qb, &net_b)?
        } else {
            None
        };
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
            return_per_trade: cx_return(&mode),
            pair_a,
            pair_b,
            usdt_b,
        })
    }

    /// Минимум и максимум вывода A (из API биржи). У DEX ограничений нет.
    /// Ошибки: TooSmall — меньше минимума, TooLarge — больше максимума (вид — для подбора объёма, решение 66).
    pub fn check_withdraw(&self, qty: Decimal) -> Result<(), Fail> {
        let Some(i) = &self.a.info else { return Ok(()) };
        if let Some(min) = i.withdraw_min
            && qty < min
        {
            return Err(Fail::TooSmall(format!("вывод {qty} меньше минимума {min}")));
        }
        if let Some(max) = i.withdraw_max
            && !max.is_zero()
            && qty > max
        {
            return Err(Fail::TooLarge(format!(
                "вывод {qty} больше максимума {max}"
            )));
        }
        Ok(())
    }
}

/// Возврат USDT в каждой сделке (варианты usdt_return.mode — per_trade / separate, settings/catalog/trade.rs).
fn cx_return(mode: &str) -> bool {
    mode == "per_trade"
}

/// Область настройки «площадка». Принимает: площадку. Возвращает: At для чтения настроек.
fn at(v: &VenueId) -> At<'_> {
    At {
        venue: Some(&v.0),
        coin: None,
    }
}
