//! Файл: venues/bsc_dex/check/quoter_amount.rs
//! Что делает: решение 261, часть 2 — сумма периодической сверки стороны пула с квотером. Тигран 12.11: «Если полученное
//! число меньше, чем наша настройка, то берем полученное число, то есть значение ликвидности в пуле. Если ликвидность
//! больше, чем наш максимум, то мерим на наш максимум»: по каждой стороне (покупка, продажа — независимо, решение 73)
//! сумма = min(наш максимум, ликвидность стороны пула), в USDT.
//! - Наш максимум — наибольший money.max_per_trade_usd среди направлений, где участвует DEX (новой настройки нет):
//!   у каждой биржи шлюза, привязанной к кошельку DEX на экране «Маппинг кошельков» (решение 132: без привязки биржа с
//!   DEX не торгует), — «биржа → DEX» и «DEX → биржа»; «DEX ↔ DEX» — если привязана его строка. Чтение — как у ядра
//!   (решение 242, core/limits_parts.rs и core/cycle_plan.rs): значение пары «площадка · тип», иначе площадка покупки
//!   (биржа → DEX — биржа, DEX → биржа и DEX ↔ DEX — DEX), иначе вся система. Не задано ни у одного — «не задана
//!   настройка» (сторона ждёт её, settings_wait.rs).
//! - Ликвидность стороны — глубина пула в полосе dex.depth_band_pct (решение 73, pools/depth_book.rs, quote/depth_band.rs)
//!   в USDT, без комиссии пула: покупка — сколько USDT пул примет, продажа — сколько USDT отдаст до границы полосы.
//! - Решение 393: мелкая сторона (сверяется, если её пропускает сумма пробы) — сумма, которую она пропускает целиком
//!   (check.probe_usdt или её доля, quote/side_fit.rs), не больше нашего максимума.
//! - Пул с крючком B/C — прежняя сумма check.probe_usdt: влияние крючка (check/hooks_probe.rs) измерено пробой именно
//!   этой суммы, и вычесть его на другой сумме по факту кода нельзя (вопрос Тиграну — docs/TASKS.md).
//!
//! Сумма в USDT переводится во встречный токен пула (решение 52 — промежуточная монета, quote/rate.rs) вызывающим.
//! Кто вызывает: check/quoter_verify.rs.
//! От чего зависит: settings (Store: decimal_dir, venue_kinds, accounts), pools/depth_book.rs, pools/listen.rs (Pools), quote/side_fit.rs (мелкая сторона, решение 393).
//! Настройки: money.max_per_trade_usd (области «площадка · тип», площадка, вся система), dex.depth_band_pct,
//! dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct (глубина), check.probe_usdt (пулы с крючком B/C).

use rust_decimal::Decimal;

use crate::core::types::{HookGroup, Side};
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::settings::accounts::ROW_DEX_DEX;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::settings::venue_dir::{DirKind, VenueDir};

use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;

/// Имя настройки максимума на сделку.
pub const MAX: &str = "money.max_per_trade_usd";

/// Отказ: причина и имя незаданной настройки (как check/quoter_verify.rs Fail).
pub type Fail = (String, Option<String>);

/// Сумма сверки стороны: USDT и откуда взята (для причины и алерта).
#[derive(Debug, Clone, PartialEq)]
pub struct Amount {
    pub usd: Decimal,
    pub source: String,
    pub physical_input: Option<alloy_primitives::U256>,
}

/// Наибольший money.max_per_trade_usd среди направлений с DEX (см. шапку). Принимает: хранилище, id DEX, биржи
/// направлений с DEX, есть ли «DEX ↔ DEX». Возвращает: максимум. Ошибки: NotConfigured (не задан ни у одного
/// направления, setting = money.max_per_trade_usd), Internal (значение в неверном виде).
pub fn max_of(
    s: &Store,
    dex: &str,
    cexes: &[String],
    dex_dex: bool,
) -> Result<Decimal, GatewayError> {
    fold_of(s, MAX, (dex, cexes, dex_dex), Decimal::max)?.ok_or_else(|| {
        GatewayError::not_set(
            None,
            MAX,
            format!(
                "Не задана настройка {MAX} ни у одного направления с DEX (биржи с кошельком DEX: {}; DEX ↔ DEX: {}) — \
                 сумма сверки с квотером не считается",
                if cexes.is_empty() { "нет".to_string() } else { cexes.join(", ") },
                if dex_dex { "есть" } else { "нет" }
            ),
        )
    })
}

/// Значение настройки `name` по областям направлений с DEX (как max_of), сведённое `pick` (наибольшее, наименьшее).
/// Принимает: хранилище, имя, (id DEX, биржи, есть ли «DEX ↔ DEX»), правило. Возвращает: None — не задано ни у
/// одного направления. Ошибки: Internal (значение в неверном виде).
pub fn fold_of(
    s: &Store,
    name: &str,
    (dex, cexes, dex_dex): (&str, &[String], bool),
    pick: fn(Decimal, Decimal) -> Decimal,
) -> Result<Option<Decimal>, GatewayError> {
    let mut asks: Vec<(VenueDir, &str)> = Vec::new();
    for c in cexes {
        asks.push((vd(c, DirKind::CexToDex), c));
        asks.push((vd(c, DirKind::DexToCex), dex));
    }
    if dex_dex {
        asks.push((vd(dex, DirKind::DexDex), dex));
    }
    let mut best: Option<Decimal> = None;
    for (pair, venue) in asks {
        let at = At {
            venue: Some(venue),
            coin: None,
        };
        match s.decimal_dir(name, Some(pair), at) {
            Ok(v) => best = Some(best.map_or(v, |b| pick(b, v))),
            Err(e) if e.code == ErrorCode::NotConfigured => {}
            Err(e) => return Err(e),
        }
    }
    Ok(best)
}

/// Пара «площадка · тип». Ошибок нет.
fn vd(venue: &str, kind: DirKind) -> VenueDir<'_> {
    VenueDir { venue, kind }
}

/// Наш максимум по живому хранилищу: биржи шлюза с привязкой к кошельку DEX и строка «DEX ↔ DEX». Ошибки: как у
/// max_of; виды площадок ещё не сообщены ядром — причина без настройки.
pub fn max_trade_usd(p: &Pools) -> Result<Decimal, Fail> {
    let s = &p.ctx.settings;
    let cexes = s.venue_kinds().cexes().ok_or((
        "виды площадок шлюза ещё не сообщены ядром — наш максимум на сделку не считается"
            .to_string(),
        None,
    ))?;
    let acc = s.accounts();
    let mapped: Vec<String> = cexes.into_iter().filter(|c| acc.row(c).is_some()).collect();
    max_of(s, &p.ctx.venue().0, &mapped, acc.row(ROW_DEX_DEX).is_some())
        .map_err(|e| (e.message, e.setting))
}

/// Ревью Н5 (лесенка влияния): наименьший signal.probe_usd среди направлений с переводом и DEX — тех же областей,
/// что читает сигнал (решение 260, core/signal_probe.rs: «площадка · тип», площадка покупки, вся система; у «DEX ↔
/// DEX» настройки нет). Возвращает: None — не задан ни у одного (сигнал тогда считает на потолке). Ошибки: как у
/// max_trade_usd.
pub fn probe_low(p: &Pools) -> Result<Option<Decimal>, Fail> {
    let s = &p.ctx.settings;
    let cexes = s.venue_kinds().cexes().ok_or((
        "виды площадок шлюза ещё не сообщены ядром — объём проверки сигнала не считается"
            .to_string(),
        None,
    ))?;
    let acc = s.accounts();
    let mapped: Vec<String> = cexes.into_iter().filter(|c| acc.row(c).is_some()).collect();
    let dex = p.ctx.venue().0;
    fold_of(s, "signal.probe_usd", (&dex, &mapped, false), Decimal::min)
        .map_err(|e| (e.message, e.setting))
}

/// Сумма стороны = min(наш максимум, глубина стороны в полосе), USDT (чистая, тестируется). Ошибок нет.
pub fn pick(max: Decimal, depth: Decimal) -> Amount {
    if depth < max {
        Amount {
            usd: depth,
            physical_input: None,
            source: format!("ликвидность стороны в полосе {depth} USDT < максимума {max} ({MAX})"),
        }
    } else {
        Amount {
            usd: max,
            physical_input: None,
            source: format!("наш максимум {max} USDT ({MAX}), ликвидность стороны {depth} USDT"),
        }
    }
}

/// Пул с крючком группы B/C — сверка прежней суммой check.probe_usdt (см. шапку). Ошибок нет.
pub fn hooked(e: &Entry) -> bool {
    e.pool
        .hooks
        .as_ref()
        .is_some_and(|h| h.group != HookGroup::A)
}

/// Сумма сверки стороны пула в USDT (см. шапку). Принимает: пулы, запись, знаки USDT, сторону. Ошибки: причина и имя
/// незаданной настройки.
pub fn side_usd(p: &Pools, e: &Entry, usdt_dec: u8, side: Side) -> Result<Amount, Fail> {
    if hooked(e) {
        let v = p
            .ctx
            .dec("check.probe_usdt")
            .map_err(|e| (e.message, e.setting))?;
        return Ok(Amount {
            usd: v,
            physical_input: None,
            source: format!("check.probe_usdt {v} USDT (пул с крючком B/C)"),
        });
    }
    let max = max_trade_usd(p)?;
    let d = p
        .reg
        .depth()
        .depth(&p.ctx, &p.reg, &p.tokens, e, usdt_dec)
        .map_err(|why| {
            (
                why,
                super::super::pools::depth_book::missing_setting(&p.ctx),
            )
        })?;
    let (depth, deep) = match side {
        Side::Buy => (d.band.buy, d.deep_buy),
        Side::Sell => (d.band.sell, d.deep_sell),
    };
    // Решение 393 (t178): мелкая сторона, которую пропускает сумма пробы, — сверка на той сумме, что сторона пропускает
    // целиком (check.probe_usdt или её доля, quote/side_fit.rs), не больше нашего максимума: глубина в полосе у неё
    // бывает почти 0 (цена в «дыре» между позициями), а котировка берёт её по всей кривой.
    if !deep
        && let Some((raw, usd)) =
            super::super::quote::side_fit::side_usd(&p.ctx, &p.reg, &p.tokens, usdt_dec, e, side)
    {
        if let crate::core::types::PoolState::Brokerbot(s) = &e.pool.state {
            let usdt = p.tokens.usdt(&p.ctx).map_err(|e|(e.message,e.setting))?;
            let ceiling = max.min(p.ctx.dec("check.probe_usdt").map_err(|e|(e.message,e.setting))?);
            let units = super::super::num::to_units(ceiling,usdt_dec).ok_or_else(||("неверная сумма сверки Aktionariat".into(),None))?;
            let cap=super::super::quote::rate::from_usdt(&p.ctx,&p.reg,usdt,s.cash,units).map_err(|e|(e.message,e.setting))?;
            if super::super::quote::brokerbot_size::one(e,side,cap)==Some(raw) {
                let input=if side==Side::Buy {raw} else {alloy_primitives::U256::ONE};
                return Ok(Amount {usd:ceiling,physical_input:Some(input),source:format!("предел для сверки одной целой акции, вход {input} атомов")});
            }
        }
        return Ok(Amount {
            usd: usd.min(max),
            physical_input: None,
            source: format!(
                "мелкая сторона (в полосе {depth} USDT): сумма пробы, которую сторона пропускает целиком, {usd} USDT (решение 393)"
            ),
        });
    }
    Ok(pick(max, depth))
}
