//! Файл: core/fit_volume.rs
//! Что делает: решение 294 (Тигран 15.11: потолок сделки — предел, а не обязательный объём) — сигнал (шаг 3,
//! core/prices.rs), когда котировка стороны на объёме проверки «не помещается»: ошибка шлюза NoRoute с признаком
//! too_large (gateway/errors.rs; адаптер ставит его только там, где причина — объём, например путь DEX через
//! промежуточную монету, шаг которого не исполним целиком в прочитанной карте тиков — quote/route_calc.rs). Постоянные
//! NoRoute (пулов нет, только сигнал, налог забирает всё, пул на паузе) признака не несут — повторов нет, ни одной
//! лишней котировки. Ядро повторяет котировку обеих сторон на объёме вдвое меньше (деление пополам — это и есть
//! операция, которую считает signal.fit_halvings, «сколько раз делить пополам»; делитель 2 — её определение, не
//! порог), не больше signal.fit_halvings попыток и не ниже минимумов площадок (signal_probe.rs floor_of): если
//! очередная половина ниже минимума, а сам минимум меньше текущего объёма, — одна попытка на минимуме, потом отказ.
//! Первый помещающийся объём — объём сравнения (дальше как при неполной глубине, решение 87). Другие ошибки — как
//! раньше. Настройка не задана — повторов нет, поведение побайтно прежнее.
//! Нагрузка (решения 129, 246): повторы — только у направления с NoRoute «не помещается»; памяти «пул не менялся — не
//! повторять» нет (quote_cache.rs годится только при том же объёме) — замер нагрузки после выкладки в docs/TASKS.md.
//! Здесь же QuoteErr — ошибка котировки с признаком «не помещается» (prices.rs, final_fit.rs).
//! Кто вызывает: core/prices.rs (QuoteErr, retry_boxed), core/final_fit.rs (QuoteErr), тесты (halve, QuoteErr —
//! tests/core_fit_volume_294.rs).
//! От чего зависит: gateway/errors.rs (too_large), fit_settings.rs, signal_probe.rs (floor_of), prices.rs (tick_quotes).
//! Настройки: signal.fit_halvings (через fit_settings.rs).

use std::future::Future;
use std::pin::Pin;

use rust_decimal::Decimal;

use crate::core::types::{Direction, Quote};
use crate::gateway::errors::{ErrorCode, GatewayError};

use super::engine::Core;

/// Ошибка котировки стороны: текст (как раньше — «площадка: причина») и признак «объём не помещается».
#[derive(Debug, Clone)]
pub struct QuoteErr {
    pub text: String,
    /// true — NoRoute с признаком too_large: меньший объём может пройти.
    pub too_large: bool,
}

impl QuoteErr {
    /// Что делает: ошибка шлюза площадки → QuoteErr. Текст — как раньше («площадка: причина»).
    /// Принимает: площадку (текст), ошибку шлюза. Возвращает: QuoteErr. Ошибок нет.
    pub fn of(venue: &str, e: &GatewayError) -> Self {
        QuoteErr {
            text: format!("{venue}: {}", e.message),
            too_large: e.code == ErrorCode::NoRoute && e.too_large,
        }
    }
}

impl From<String> for QuoteErr {
    /// Ошибка без кода шлюза (нет кошелька в маппинге и т.п.) — не «не помещается».
    fn from(text: String) -> Self {
        QuoteErr {
            text,
            too_large: false,
        }
    }
}

impl From<QuoteErr> for String {
    /// Текст причины — как раньше (prices.rs отдаёт String).
    fn from(e: QuoteErr) -> Self {
        e.text
    }
}

/// Что делает: повтор котировки на меньшем объёме, пока не поместится (см. шапку). Принимает: объём, на котором
/// котировка не поместилась, минимум площадок (монет; 0 — нет), число попыток (> 0), первую ошибку, котировку на объём.
/// Возвращает: (котировка, объём, на котором она получена). Ошибки: первая ошибка без признака too_large — она же,
/// без единой котировки; другая ошибка повтора — она как есть; не нашлось — первая ошибка с припиской.
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
    if !first.too_large {
        return Err(first);
    }
    let (mut qty, mut tried, mut last) = (want, 0u32, None);
    while tried < tries {
        // Деление пополам — определение операции signal.fit_halvings (см. шапку).
        let half = qty / Decimal::TWO;
        let next = if half >= floor {
            half
        } else if floor < qty {
            floor
        } else {
            break;
        };
        if next <= Decimal::ZERO {
            break;
        }
        qty = next;
        tried += 1;
        match quote(qty).await {
            Ok(t) => return Ok((t, qty)),
            Err(e) if e.too_large => last = Some(qty),
            Err(e) => return Err(e),
        }
        if qty == floor {
            break;
        }
    }
    let note = match last {
        Some(q) => format!(
            "; меньший объём не помещается: пробовали до {} (попыток {tried}, минимум площадок {})",
            q.normalize(),
            floor.normalize()
        ),
        None => format!(
            "; меньше объёма нельзя — минимум площадок {}",
            floor.normalize()
        ),
    };
    Err(QuoteErr {
        text: format!("{}{note}", first.text),
        too_large: true,
    })
}

/// Будущее повтора сигнала в куче (retry_boxed).
type Retry<'a> =
    Pin<Box<dyn Future<Output = Result<(Quote, Quote, Decimal), QuoteErr>> + Send + 'a>>;

/// Что делает: retry в куче — будущее собирается здесь, а не в кадре расчёта сигнала (стек не растёт; как
/// precheck_fast.rs recalc_boxed). Принимает и ошибки: как retry.
pub(super) fn retry_boxed<'a>(
    core: &'a Core,
    dir: &'a Direction,
    ref_price: Decimal,
    want: Decimal,
    first: QuoteErr,
) -> Retry<'a> {
    Box::pin(retry(core, dir, ref_price, want, first))
}

/// Что делает: сигнал (prices.rs for_direction) — первая котировка направления на объёме проверки не удалась.
/// Принимает: ядро, направление, опорную цену (> 0), объём проверки, ошибку первой котировки.
/// Возвращает: (покупка на A, продажа на B, объём, на котором они получены). Ошибки: та же ошибка, если она не
/// «не помещается» или signal.fit_halvings не задана (в неверном виде — то же, с записью в журнал); иначе — как halve.
async fn retry(
    core: &Core,
    dir: &Direction,
    ref_price: Decimal,
    want: Decimal,
    first: QuoteErr,
) -> Result<(Quote, Quote, Decimal), QuoteErr> {
    if !first.too_large {
        return Err(first);
    }
    let Some(n) = super::fit_settings::tries(core, "signal.fit_halvings").await else {
        return Err(first);
    };
    let floor = super::signal_probe::floor_of(core, dir, ref_price);
    let quote = |q: Decimal| super::prices::tick_quotes(core, dir, q);
    let ((qa, qb), qty) = halve(want, floor, n, first, quote).await?;
    Ok((qa, qb, qty))
}
