//! Файл: core/fit_volume.rs
//! Что делает: решение 294 (Тигран 15.11: потолок сделки — предел, а не обязательный объём) — сигнал (шаг 3,
//! core/prices.rs) при «объём проверки не помещается» у котировки стороны. Котировка стороны на объёме проверки
//! вернула отказ с кодом шлюза NoRoute (то же правило, что у подбора объёма: costs_quote.rs classify — NoRoute =
//! «объём слишком большой», TooLarge; у DEX это, например, путь через промежуточную монету, шаг которого не исполним
//! целиком в прочитанной карте тиков — quote/route_calc.rs) — ядро повторяет котировку обеих сторон на объёме вдвое
//! меньше, пока объём не меньше минимумов площадок (signal_probe.rs floor_of), не больше signal.fit_halvings попыток.
//! Первый помещающийся объём — объём сравнения (дальше как при неполной глубине, решение 87). Любая другая ошибка —
//! как раньше (сигнала нет). Настройка не задана — повторов нет, поведение побайтно прежнее.
//! Ядро о площадках не знает: признак «не помещается» — только код ошибки шлюза.
//! Нагрузка (решения 129, 246): повторы — только у направления, чья первая котировка дала NoRoute; памяти «пул не
//! менялся — не повторять» нет (памяти котировок quote_cache.rs годятся только при том же объёме) — замер нагрузки
//! после выкладки записан в docs/TASKS.md.
//! Здесь же QuoteErr — ошибка котировки с признаком «не помещается» (prices.rs, profit.rs final_fillable) и чтение
//! необязательного числа попыток (signal.fit_halvings, size.final_fit_steps — final_fit.rs).
//! Кто вызывает: core/prices.rs (QuoteErr, retry), core/profit.rs (QuoteErr, tries), тесты (halve —
//! tests/core_fit_volume_294.rs).
//! От чего зависит: costs_quote.rs (classify), signal_probe.rs (floor_of), prices.rs (tick_quotes), settings.
//! Настройки: signal.fit_halvings (вся система; необязательная).

use std::future::Future;

use rust_decimal::Decimal;

use crate::core::types::{Direction, Quote};
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::Core;
use super::size::Fail;

/// Ошибка котировки стороны: текст (как раньше — «площадка: причина») и признак «объём не помещается».
#[derive(Debug, Clone)]
pub struct QuoteErr {
    pub text: String,
    /// true — код шлюза NoRoute (меньший объём может пройти; costs_quote.rs classify → TooLarge).
    pub no_route: bool,
}

impl QuoteErr {
    /// Что делает: ошибка шлюза площадки → QuoteErr тем же правилом, что подбор объёма (costs_quote.rs classify).
    /// Принимает: площадку (текст), ошибку шлюза. Возвращает: QuoteErr. Ошибок нет.
    pub fn of(venue: &str, e: &GatewayError) -> Self {
        match super::costs_quote::classify(venue, e) {
            Fail::TooLarge(text) => QuoteErr {
                text,
                no_route: true,
            },
            f => QuoteErr {
                text: f.to_string(),
                no_route: false,
            },
        }
    }
}

impl From<String> for QuoteErr {
    /// Ошибка без кода шлюза (нет кошелька в маппинге и т.п.) — не «не помещается».
    fn from(text: String) -> Self {
        QuoteErr {
            text,
            no_route: false,
        }
    }
}

impl From<QuoteErr> for String {
    /// Текст причины — как раньше (prices.rs, profit.rs отдают String).
    fn from(e: QuoteErr) -> Self {
        e.text
    }
}

/// Что делает: читает необязательное число попыток (вся система). Принимает: хранилище, имя настройки.
/// Возвращает: Some(n) или None — не задана. Ошибки: текст — значение хранится в неверном виде.
pub fn tries(s: &Store, name: &str) -> Result<Option<u32>, String> {
    match s.integer(name, At::default()) {
        Ok(v) => Ok(Some(u32::try_from(v).unwrap_or(u32::MAX))),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e.message),
    }
}

/// Что делает: повтор котировки на объёме вдвое меньше, пока не поместится. Принимает: объём, на котором котировка не
/// поместилась, минимум площадок (монет; 0 — нет), число попыток (> 0), первую ошибку, котировку на объём.
/// Возвращает: (котировка, объём, на котором она получена). Ошибки: другая ошибка котировки — она как есть; объём
/// ушёл ниже минимума или попытки кончились — первая ошибка с припиской, до какого объёма пробовали.
pub async fn halve<T, F, Fut>(
    want: Decimal,
    floor: Decimal,
    tries: u32,
    first: QuoteErr,
    mut quote: F,
) -> Result<(T, Decimal), QuoteErr>
where
    F: FnMut(Decimal) -> Fut,
    Fut: Future<Output = Result<T, QuoteErr>>,
{
    let two = Decimal::TWO;
    let (mut qty, mut tried, mut last) = (want, 0u32, None);
    while tried < tries {
        let next = qty / two;
        if next <= Decimal::ZERO || next < floor {
            break;
        }
        qty = next;
        tried += 1;
        match quote(qty).await {
            Ok(t) => return Ok((t, qty)),
            Err(e) if e.no_route => last = Some(qty),
            Err(e) => return Err(e),
        }
    }
    let note = match last {
        Some(q) => format!(
            "; меньший объём не помещается: пробовали до {} (попыток {tried}, signal.fit_halvings / минимум площадок {})",
            q.normalize(),
            floor.normalize()
        ),
        None => format!(
            "; половина объёма ниже минимума площадок {} — повторов нет",
            floor.normalize()
        ),
    };
    Err(QuoteErr {
        text: format!("{}{note}", first.text),
        no_route: true,
    })
}

/// Что делает: сигнал (prices.rs for_direction) — первая котировка направления на объёме проверки не удалась.
/// Принимает: ядро, направление, опорную цену (> 0), объём проверки, ошибку первой котировки.
/// Возвращает: (покупка на A, продажа на B, объём, на котором они получены). Ошибки: та же ошибка, если она не
/// «не помещается» или signal.fit_halvings не задана; иначе — как у halve; настройка в неверном виде — её текст.
pub(super) async fn retry(
    core: &Core,
    dir: &Direction,
    ref_price: Decimal,
    want: Decimal,
    first: QuoteErr,
) -> Result<(Quote, Quote, Decimal), QuoteErr> {
    if !first.no_route {
        return Err(first);
    }
    let Some(n) = tries(&core.inner.deps.settings, "signal.fit_halvings")? else {
        return Err(first);
    };
    let floor = super::signal_probe::floor_of(core, dir, ref_price);
    let quote = |q: Decimal| super::prices::tick_quotes(core, dir, q);
    let ((qa, qb), qty) = halve(want, floor, n, first, quote).await?;
    Ok((qa, qb, qty))
}
