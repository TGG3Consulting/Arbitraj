//! Файл: core/daily_value.rs
//! Что делает: оценка позиции, которую бот сам не довёл (решение 152): остановленная сделка (Halted) и «Беру на себя».
//! Оценка = выручка + стоимость монет сделки по рынку площадки, где они лежат (котировка продажи всего количества,
//! чистыми — минус комиссии вне цены, trade_ops::sell_value) − вложено − затраты. Монеты больше не считаются
//! потерянными целиком: позиция, которая стоит ≈ вложенному, даёт оценку ≈ 0, а не «минус весь ордер».
//! Цены нет (площадка не отвечает, нет стакана или пула, глубины не хватает на всё количество) — оценка неизвестна:
//! для итога дня и лимита берётся худший случай (монеты = 0, то есть вся вложенная сумма), с явной пометкой
//! «цены нет — худший случай» в сделке, в итоге дня (DayView.unpriced) и на плашке. Монет в сделке нет — итог по
//! известным суммам (выручка − вложено − затраты), это не оценка, а уже известный результат без монет.
//! Кто вызывает: core/daily.rs (provisional).
//! От чего зависит: deal.rs, trade_ops.rs (sell_value — котировка шлюза).
//! Настройки: нет (котировка — по настройкам площадки: venue.quote_asset, exec.dex_slippage_pct — trade_ops.rs).

use rust_decimal::Decimal;

use super::deal::Deal;
use super::engine::Core;

/// Оценка позиции для итога дня.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    /// Сумма в итог дня, USDT (обычно ≤ 0: убыток, если продать сейчас).
    pub amount: Decimal,
    /// true — посчитано по рыночной цене или монет нет; false — цены нет, взят худший случай (вся сумма).
    pub priced: bool,
    /// Текст для хронологии сделки: из чего сложилась сумма.
    pub note: String,
}

/// Что делает: стоимость монет сделки по рынку площадки, где они лежат (deal.at), чистыми.
/// Принимает: ядро, сделку. Возвращает: USDT. Ошибки: монет нет, площадки нет в шлюзе, нет котировки, мало глубины.
pub async fn value_now(core: &Core, deal: &Deal) -> Result<Decimal, String> {
    if deal.qty <= Decimal::ZERO {
        return Err("монет в сделке нет".into());
    }
    super::trade_ops::deal_value(core, deal)
        .await
        .map(|(v, _)| v)
}

/// Что делает: оценка позиции по стоимости монет (решение 152).
/// Принимает: сделку, стоимость её монет по рынку (Err — причина, почему цены нет).
/// Возвращает: оценку (сумма, есть ли цена, заметка). Ошибок нет.
pub fn estimate(deal: &Deal, value: Result<Decimal, String>) -> Estimate {
    // Решение 164: числа для глаз (деньги — 2 знака, количество — без длинного хвоста), площадка — по имени.
    use crate::notify::human::{money as m, qty as q, venue as v_name};
    let known = deal.proceeds - deal.invested - deal.spent;
    let t = &deal.direction.asset.ticker;
    if deal.qty <= Decimal::ZERO {
        return Estimate {
            amount: known,
            priced: true,
            note: format!(
                "монет в сделке нет — итог по известным суммам: выручка {} − вложено {} − затраты {} = {} USDT",
                m(deal.proceeds),
                m(deal.invested),
                m(deal.spent),
                m(known)
            ),
        };
    }
    match value {
        Ok(v) => {
            let amount = deal.result_if(v);
            Estimate {
                amount,
                priced: true,
                note: format!(
                    "оценка позиции: {} {t} на {} по рынку стоят {} USDT чистыми; выручка {} − вложено {} − затраты {} → {} USDT",
                    q(deal.qty),
                    v_name(&deal.at.0),
                    m(v),
                    m(deal.proceeds),
                    m(deal.invested),
                    m(deal.spent),
                    m(amount)
                ),
            }
        }
        Err(why) => Estimate {
            amount: known,
            priced: false,
            note: format!(
                "цены нет ({why}) — оценка неизвестна, для итога дня и лимита взят худший случай: {} {t} на {} = 0, \
                 то есть {} USDT",
                q(deal.qty),
                v_name(&deal.at.0),
                m(known)
            ),
        },
    }
}
