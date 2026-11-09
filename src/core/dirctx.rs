//! Файл: core/dirctx.rs
//! Что делает: всё, что нужно расчёту направления и не зависит от объёма: площадки и их данные о сети
//! BSC, сеть для вывода и депозита, котируемые монеты, адрес USDT, σ на B, время в пути T, k, h,
//! режим возврата USDT, налог перевода при выводе с биржи (решение 118), минимумы вывода и минимальный депозит, правила пар бирж (шаг количества, минимальная и
//! максимальная сумма ордера — решение 66). Собирается один раз на кандидата.
//! Время в пути — transit.rs (решение 50); σ — по окну profit.sigma_window_min (решение 49, vol.rs).
//! Решение 237: build_decided — контекст на момент решения о входе с σ и T из плана сделки (core/frozen.rs take).
//! Решение 240 (reserve.rs): build — σ с проверкой profit.sigma_min_changes и границей profit.sigma_floor_per_s [B],
//! доля закрытия разницы profit.closure_share [B]; build_decided — σ из плана, доля и все настройки контекста — из копии
//! настроек на момент входа (решение 239, Decided.settings); build_held (частичная покупка, сделка уже идёт) — как до
//! решения 240: σ ряда без проверки и границы, без доли.
//! Кто вызывает: core/profit.rs, card_max.rs, uncapped.rs (кандидаты) — build; partial.rs — build_held; frozen.rs —
//! build_decided.
//! От чего зависит: engine.rs, registry.rs, vol.rs, reserve.rs, settings.
//! Правила пар и свежесть справочника сетей — dirctx_venue.rs (venue.networks_stale_s).
//! Решение 242: profit.price_move_k и profit.closure_share — сначала площадка + тип направления (core/venue_dir.rs).
//! Настройки: venue.quote_asset, profit.price_move_k (можно по монете), profit.liquidity_share_h (по B),
//! profit.sigma_window_min, usdt_return.mode (+ transit.rs), profit.closure_share, profit.sigma_floor_per_s,
//! profit.sigma_min_changes (reserve.rs).

use rust_decimal::Decimal;

use crate::core::types::{Asset, CoinNetworkInfo, Direction, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::events::SpotPairState;
use crate::settings::read::At;
use crate::settings::store::Store;

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
    /// σ продажи на B за секунду; решение 240 — с границей profit.sigma_floor_per_s (reserve.rs sigma_now).
    pub sigma: Decimal,
    pub transit_s: Decimal,
    pub k: Decimal,
    /// Решение 240: доля закрытия разницы profit.closure_share [B]; None — не задана (запас как раньше).
    pub closure_share: Option<Decimal>,
    pub h: Decimal,
    pub return_per_trade: bool,
    /// Правила спот-пары биржи на A и на B (решение 66); у DEX — None.
    pub pair_a: Option<SpotPairState>,
    pub pair_b: Option<SpotPairState>,
    /// Сеть BSC котируемой монеты B у биржи B (комиссия вывода — для возврата USDT без запроса к бирже, решение 66);
    /// у DEX — None.
    pub usdt_b: Option<CoinNetworkInfo>,
    /// Налог перевода монеты при выводе с биржи A, % (решение 118, Я-3, transfer_tax.rs); у DEX на A — None (налог
    /// даёт шлюз DEX в стоимости перевода).
    pub transfer_tax: Option<Decimal>,
}

/// Решение 240: момент сборки контекста — от него зависят σ, доля закрытия и хранилище настроек.
enum Moment<'a> {
    /// План входа (кандидат, пересчёт перед покупкой): σ с проверкой и границей, доля — живые настройки.
    Entry,
    /// Сделка уже идёт (частичная покупка): как до решения 240 — σ ряда, без доли.
    Held,
    /// Момент решения о входе: σ и T плана, настройки — копия на этот момент (решения 237, 239).
    Decided {
        sigma: Decimal,
        transit_s: Decimal,
        settings: &'a Store,
    },
}

impl DirCtx {
    /// Собирает контекст для плана входа. Ошибки: текст причины (монеты или площадки нет в справочнике, не задана
    /// настройка, σ ещё не измерена (решение 240: и ненулевых изменений цены меньше profit.sigma_min_changes), нет сети
    /// BSC ни у одной стороны, нет адреса USDT).
    pub fn build(core: &Core, dir: &Direction) -> Result<DirCtx, String> {
        Self::build_with(core, dir, Moment::Entry)
    }

    /// Решение 240: контекст для сделки, которая уже идёт (частичная покупка, partial.rs) — как до решения 240: σ ряда
    /// без проверки profit.sigma_min_changes и границы, без доли закрытия. Ошибки: как у build.
    pub fn build_held(core: &Core, dir: &Direction) -> Result<DirCtx, String> {
        Self::build_with(core, dir, Moment::Held)
    }

    /// Решение 237: контекст на момент решения о входе (core/frozen.rs take) — σ и время в пути T берутся из плана
    /// сделки (те же, что у плана входа), а не измеряются заново; всё прочее — как у build, в тот же момент.
    /// Решение 240: настройки (k, h, доля закрытия и прочие) — из копии на момент входа `settings` (решение 239).
    /// Принимает: ядро, направление, σ и T плана, копию настроек. Ошибки: как у build (кроме σ и T).
    pub fn build_decided(
        core: &Core,
        dir: &Direction,
        sigma: Decimal,
        transit_s: Decimal,
        settings: &Store,
    ) -> Result<DirCtx, String> {
        Self::build_with(
            core,
            dir,
            Moment::Decided {
                sigma,
                transit_s,
                settings,
            },
        )
    }

    /// Сборка контекста на момент `at_m` (см. Moment). Ошибки: как у build.
    fn build_with(core: &Core, dir: &Direction, at_m: Moment) -> Result<DirCtx, String> {
        let s = match &at_m {
            Moment::Decided { settings, .. } => *settings,
            _ => &core.inner.deps.settings,
        };
        let coins = rd(&core.inner.coins);
        // Решение 131: по карте места, без прохода справочника.
        let entry = super::ticker_index::find(core, &coins, &dir.asset)
            .ok_or("монеты нет в справочнике")?;
        let a = entry.at(&dir.buy_on).ok_or("A нет у монеты")?.clone();
        let b = entry.at(&dir.sell_on).ok_or("B нет у монеты")?.clone();
        // Решение 108: у каждой площадки своё название сети BSC; у DEX (нет сети в справочнике) — своя
        // venue.bsc_network, и только потом название другой стороны.
        let own = |v: &VenueId| super::transfers::own_bsc_name(core, v);
        let net_a = a
            .network
            .clone()
            .or_else(|| own(&dir.buy_on))
            .or(b.network.clone());
        let net_b = b
            .network
            .clone()
            .or_else(|| own(&dir.sell_on))
            .or(a.network.clone());
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
        // σ продажи на B — ряд этого направления (напарник A, решение 118); решение 237 — у плана сделки своя;
        // решение 240 — у плана входа с проверкой и границей (reserve.rs).
        let sigma = match &at_m {
            Moment::Decided { sigma, .. } => *sigma,
            Moment::Entry => {
                super::reserve::sigma_now(&mut lk(&core.inner.vol), s, dir, chrono::Utc::now())?
            }
            Moment::Held => {
                let window = s
                    .integer("profit.sigma_window_min", At::default())
                    .map_err(|e| e.message)?;
                lk(&core.inner.vol).sigma(
                    &dir.sell_on,
                    &dir.asset,
                    Side::Sell,
                    &dir.buy_on,
                    window,
                    chrono::Utc::now(),
                )?
            }
        };
        let coin =
            crate::settings::scope::coin_id(&dir.asset.ticker, dir.asset.bsc_contract.as_ref());
        let coin_at = At {
            venue: None,
            coin: Some(&coin),
        };
        // Решение 242: k и доля закрытия — площадки и типа направления (venue_dir.rs), иначе прежняя цепочка.
        let vd = super::venue_dir::of(core, dir);
        let k = s
            .decimal_dir("profit.price_move_k", vd, coin_at)
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
        let transit_s = match &at_m {
            Moment::Decided { transit_s, .. } => *transit_s,
            _ => super::transit::seconds(core, dir)?,
        };
        // Решение 240: доля закрытия разницы — не у сделки, которая уже идёт.
        let closure_share = match at_m {
            Moment::Held => None,
            _ => super::reserve::closure_share(s, dir, vd)?,
        };
        // Решение 211: пара площадки — по её тикеру монеты (Presence::ticker).
        let pair_a = venue::pair_rules(core, &dir.buy_on, &a.ticker)?;
        let pair_b = venue::pair_rules(core, &dir.sell_on, &b.ticker)?;
        // Решение 66: комиссии вывода и возврата — из последнего справочника сетей ядра (не из копии в справочнике
        // монет); справочник старше venue.networks_stale_s — сделки нет.
        let latest = |p: &Presence, v: &VenueId, n: &str| -> Result<Option<_>, String> {
            match p.info {
                None => Ok(None),
                Some(_) => venue::network(core, v, &dir.asset, n)
                    .ok_or(format!(
                        "{}: сети {n} у {} нет в справочнике сетей",
                        v.0, p.ticker
                    ))
                    .and_then(|i| venue::fresh(core, &i).map(|_| Some(i))),
            }
        };
        let mut a = a;
        a.info = latest(&a, &dir.buy_on, &net_a)?;
        let mut b = b;
        b.info = latest(&b, &dir.sell_on, &net_b)?;
        // Решение 118 (Я-3): налог перевода при выводе с биржи; не измерен — расчёта нет (причина в тексте).
        let transfer_tax = match a.info {
            Some(_) => Some(super::transfer_tax::pct(core, &dir.sell_on, &dir.asset)?),
            None => None,
        };
        let usdt_b = if cx_return(&mode) && b.info.is_some() {
            Some(
                venue::quote_network(core, &dir.sell_on, &qb, &net_b)
                    .ok_or(format!(
                        "{}: сети {net_b} у {qb} нет в справочнике сетей",
                        dir.sell_on.0
                    ))
                    .and_then(|i| venue::fresh(core, &i).map(|_| i))?,
            )
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
            closure_share,
            return_per_trade: cx_return(&mode),
            pair_a,
            pair_b,
            usdt_b,
            transfer_tax,
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
