//! Файл: core/signal_probe.rs
//! Что делает: решение 260 — объём проверки сигнала (грубый спред шага 3, core/prices.rs). Настройка signal.probe_usd
//! (необязательная, область «площадка + тип направления» решения 242: пара → площадка покупки A → вся система, как у
//! money.max_per_trade_usd). Не задана — объём сравнения ровно прежний: потолок money.max_per_trade_usd / опорная цена
//! (решение 87). Задана — min(signal.probe_usd, потолок) / опорная цена, но не меньше минимумов площадок, известных в
//! памяти ядра без запросов к сети, и не больше потолка.
//! Почему меньший объём не теряет сделок (Тигран 12.11: «тогда мы можем смело менять местами расчеты?» — «да, начинай»):
//! средняя цена сделки с объёмом только ухудшается (уровни стакана, кривая пула), процентные налоги от суммы не зависят,
//! фиксированные затраты (газ, комиссия вывода) в грубый спред не входят — спред на меньшем объёме не ниже спреда на
//! большем. Подбор объёма кандидата (решение 66, size_search.rs) идёт на всём отрезке до бюджета (≤ потолка) — от
//! объёма сигнала он не зависит (оценка старта — бюджет / цена, profit.rs).
//! Минимумы площадок на шаге сигнала (только память): пара биржи A и B — minQty и minNotional / опорная цена
//! (market_read.rs pair, справочник пар); вывод A — withdrawMin, депозит B — depositMin (справочник сетей ядра, как
//! dirctx.rs; нет в нём — копия в справочнике монет). Точный минимум с учётом комиссий вывода и шага ищет подбор
//! объёма (size_bounds.rs lower); здесь — нижняя граница, чтобы проверка не шла на объёме, который заведомо не исполнить.
//! Кто вызывает: core/prices.rs (for_direction), тесты (tests/core_signal_probe.rs).
//! От чего зависит: settings (Store::decimal_dir), market_read.rs, dirctx_venue.rs (network), registry.rs, ticker_index.rs.
//! Настройки: signal.probe_usd.

use rust_decimal::Decimal;

use crate::core::types::{CoinNetworkInfo, Direction};
use crate::gateway::errors::ErrorCode;
use crate::gateway::events::SpotPairState;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::settings::venue_dir::VenueDir;

use super::engine::{Core, rd};

/// Что делает: читает signal.probe_usd. Принимает: хранилище, пару «площадка + тип направления» (None — нет), прежнюю
/// область (площадка покупки A). Возвращает: Some(сумма, USDT) или None — не задана нигде.
/// Ошибки: текст — значение хранится в неверном виде (сигнала нет: «не торгуем вслепую»).
pub fn probe_usd(s: &Store, vd: Option<VenueDir>, at: At) -> Result<Option<Decimal>, String> {
    match s.decimal_dir("signal.probe_usd", vd, at) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e.message),
    }
}

/// Что делает: наибольший из минимумов площадок в монетах. Принимает: пары бирж A и B (None — сторона DEX или пары нет
/// в памяти), минимум вывода A и депозита B (монет; None — не сообщён или сторона DEX), опорную цену (> 0).
/// Возвращает: минимум, монет (0 — минимумов нет). Ошибок нет.
pub fn floor_qty(
    pairs: [Option<&SpotPairState>; 2],
    withdraw_min_a: Option<Decimal>,
    deposit_min_b: Option<Decimal>,
    ref_price: Decimal,
) -> Decimal {
    let mut out = Decimal::ZERO;
    for p in pairs.into_iter().flatten() {
        out = out.max(p.min_qty.unwrap_or_default());
        out = out.max(p.min_notional.unwrap_or_default() / ref_price);
    }
    out.max(withdraw_min_a.unwrap_or_default())
        .max(deposit_min_b.unwrap_or_default())
}

/// Что делает: объём проверки сигнала. Принимает: потолок money.max_per_trade_usd (USDT), signal.probe_usd (None — не
/// задана), опорную цену (> 0), минимумы площадок (монет, floor_qty).
/// Возвращает: монет — без настройки ровно потолок / опорная цена (прежний расчёт); с настройкой —
/// max(min(probe, потолок) / цена, минимумы), но не больше потолка / цена. Ошибок нет.
pub fn qty(
    cap_usd: Decimal,
    probe: Option<Decimal>,
    ref_price: Decimal,
    floor: Decimal,
) -> Decimal {
    let cap_qty = cap_usd / ref_price;
    match probe {
        None => cap_qty,
        Some(p) => (p.min(cap_usd) / ref_price).max(floor).min(cap_qty),
    }
}

/// Что делает: минимумы площадок направления из памяти ядра (пары — справочник пар, сети — справочник сетей, иначе
/// копия в справочнике монет). Принимает: ядро, направление, опорную цену (> 0). Возвращает: монет (floor_qty).
/// Ошибок нет: чего нет в памяти — не учитывается (точный минимум — подбор объёма, size_bounds.rs).
pub(super) fn floor_of(core: &Core, dir: &Direction, ref_price: Decimal) -> Decimal {
    let (a, b) = {
        let coins = rd(&core.inner.coins);
        let Some(entry) = super::ticker_index::find(core, &coins, &dir.asset) else {
            return Decimal::ZERO;
        };
        (
            entry.at(&dir.buy_on).cloned(),
            entry.at(&dir.sell_on).cloned(),
        )
    };
    // Сеть стороны: последняя из справочника сетей ядра (как dirctx.rs), иначе копия в справочнике монет.
    let info = |p: &super::registry::Presence| -> Option<CoinNetworkInfo> {
        let own = p.info.as_ref()?;
        super::dirctx_venue::network(core, &p.venue, &dir.asset, &own.network).or(Some(own.clone()))
    };
    let withdraw_min = a.as_ref().and_then(info).and_then(|i| i.withdraw_min);
    let deposit_min = b.as_ref().and_then(info).and_then(|i| i.deposit_min);
    let market = rd(&core.inner.market);
    let pair = |p: &Option<super::registry::Presence>| {
        p.as_ref()
            .and_then(|p| market.pair(&p.venue, &p.ticker).cloned())
    };
    let (pa, pb) = (pair(&a), pair(&b));
    floor_qty(
        [pa.as_ref(), pb.as_ref()],
        withdraw_min,
        deposit_min,
        ref_price,
    )
}
