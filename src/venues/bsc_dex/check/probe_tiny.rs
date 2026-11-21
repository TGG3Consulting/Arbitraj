//! Файл: venues/bsc_dex/check/probe_tiny.rs
//! Что делает: решение 393 (задача t178) — крошечный выход пробного обмена не даёт ни налога, ни вердикта. Пробу теперь
//! делают и мелкие пулы (check/taxes_util.rs probe_pools), где выход может оказаться в единицы токена: налог
//! (квотер − пришло) / квотер при выходе в несколько единиц — округление в 1 единицу даёт ложный «налог 0» или ложный
//! налог, а откат обмена с минимумом выхода 1 единица (check/probe.rs) — свойство суммы, не монеты. Порог — наименьший
//! выход quote/side_fit.rs min_units (1 единица ≤ dex.quoter_max_diff_pct % выхода). Действует на все пулы пробы.
//! - Покупка: ответ квотера («ожидалось без налога») меньше порога — помеха NoRoute («не проверено», повтор), а не
//!   замер; откат шага покупки при таком ответе квотера — тоже помеха, не вердикт «не прошла».
//! - Продажа: ответ квотера на продажу меньше порога — налог продажи не измерен (помеха, reverted = false), в том числе
//!   когда сама продажа откатилась.
//!
//! Ловушка «покупка проходит, продажа нет / налог 99 %» по-прежнему ловится: у неё ответ квотера — по кривой пула без
//! налога и не крошечный (порог — единицы токена), значит откат продажи остаётся вердиктом (probe_calls.rs sell_result),
//! а налог 99 % — замером (pool_taxes.rs).
//! Кто вызывает: check/probe.rs.
//! От чего зависит: quote/probe_quote.rs (Way: разбор ответа квотера), check/simulate.rs (CallResult), check/probe.rs
//! (SellMiss).
//! Настройки: через quote/side_fit.rs — dex.quoter_max_diff_pct [bsc_dex] (вызывающий передаёт min).

use alloy_primitives::{Address, U256};

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::super::quote::probe_quote::Way;
use super::probe::SellMiss;
use super::simulate::CallResult;

/// Текст: выход меньше наименьшего для замера. Ошибок нет.
fn small(what: &str, v: U256, min: U256) -> String {
    format!(
        "{what}: ответ квотера {v} единиц меньше наименьшего выхода для замера {min} (1 единица ≤ dex.quoter_max_diff_pct %, решение 393) — налог не измерить, это не вердикт по монете"
    )
}

/// «Ожидалось без налога» пробной покупки меньше порога — помеха. Принимает: окружение, пул, ответ квотера, порог.
/// Возвращает: Some(NoRoute) — помеха; None — замер годится. Ошибок иных нет.
pub fn buy(ctx: &Ctx, e: &Entry, expected: U256, min: U256) -> Option<GatewayError> {
    (expected < min).then(|| {
        let why = small(
            &format!("{} пробная покупка", e.family.title),
            expected,
            min,
        );
        ctx.err(ErrorCode::NoRoute, why)
    })
}

/// Откат шага пробной покупки ПОСЛЕ квотера при крошечном ответе квотера — помеха (иначе — решать как раньше,
/// probe_calls.rs buy_revert). Принимает: окружение, пул, способ «ожидалось», (номер отката, номер квотера), итоги
/// первой симуляции, (токен входа, сумма), порог. Возвращает: Some(NoRoute) или None. Ошибок иных нет.
pub fn buy_revert(
    ctx: &Ctx,
    e: &Entry,
    way: Way,
    (i, q): (usize, usize),
    first: &[CallResult],
    (token_in, amount): (Address, U256),
    min: U256,
) -> Option<GatewayError> {
    if i <= q {
        return None;
    }
    let quote = first.get(q).filter(|r| r.ok())?;
    match way.decode(ctx, e, &quote.return_data, token_in, amount) {
        Some(Ok(v)) => buy(ctx, e, v, min),
        _ => None,
    }
}

/// Итог пробной продажи с порогом (см. шапку). Принимает: окружение, пул, способ, итог квотера на продажу, сколько
/// продавалось, порог, итог probe_calls.rs sell_result. Возвращает: тот же итог; при ответе квотера меньше порога —
/// SellMiss { reverted: false } (помеха). Ошибок нет.
pub fn sell(
    ctx: &Ctx,
    e: &Entry,
    way: Way,
    quote: &CallResult,
    rest: U256,
    min: U256,
    got: Result<(U256, U256), SellMiss>,
) -> Result<(U256, U256), SellMiss> {
    let expected = match &got {
        Ok((v, _)) => Some(*v),
        Err(m) if m.reverted && quote.ok() => {
            match way.decode(ctx, e, &quote.return_data, e.coin, rest) {
                Some(Ok(v)) => Some(v),
                _ => None,
            }
        }
        Err(_) => None,
    };
    match expected {
        Some(v) if v < min => Err(SellMiss {
            reverted: false,
            why: small(&format!("{} пробная продажа", e.family.title), v, min),
        }),
        _ => got,
    }
}
