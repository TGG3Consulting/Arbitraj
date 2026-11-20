//! Файл: core/partial_back.rs
//! Что делает: решение 480 (ревью) — итог «продать на A» частичной покупки (partial.rs) с тем же худшим случаем, что у
//! продажи на B: наши продажи монеты на A с неизвестным исходом (и идущие сделки групп, которые продают на A) — впереди
//! нашей продажи как ещё не исполненные (ride_quote::ahead_noted для направления с площадкой продажи A). Сдвиг 0 —
//! котировка как раньше (trade_ops::sell_value). Иначе: котировка на (сдвиг + наш объём) и на сдвиг, наша часть —
//! ride_quote::marginal (пропорциональные затраты разностью, газ и прочие разовые — одного нашего обмена), чистыми —
//! trade_ops::net_of. Глубины A не хватает на сдвиг + наш объём — отказ «продать на A» (худший случай не проходит).
//! Кто вызывает: core/partial.rs (choose).
//! От чего зависит: ride_quote.rs, trade_ops.rs, core/types.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, Direction};

use super::deal::Deal;
use super::engine::Core;

/// Что делает: чистыми (USDT) от продажи всего количества сделки на A сейчас, после наших продаж впереди (шапка).
/// Принимает: ядро, счёт DEX сделки (None — биржа), сделку. Возвращает: (сумма, строка хронологии о худшем случае;
/// None — продаж с неизвестным исходом на A нет). Ошибки: текст — нет котировки или глубины, нет курса затрат, запись
/// сделки с неизвестным исходом не найдена (ride_settled::worst).
pub async fn value(
    core: &Core,
    acc: Option<&DexAccountId>,
    deal: &Deal,
) -> Result<(Decimal, Option<String>), String> {
    let (v, a, qty) = (&deal.direction.buy_on, &deal.direction.asset, deal.qty);
    let back = Direction {
        sell_on: v.clone(),
        ..deal.direction.clone()
    };
    let (off, note) = super::ride_quote::ahead_noted(core, &back)?;
    if off.is_zero() {
        let (x, _) = super::trade_ops::sell_value(core, acc, v, a, qty).await?;
        return Ok((x, note));
    }
    let (_, total) = super::trade_ops::sell_value(core, acc, v, a, qty + off).await?;
    let (_, ahead) = super::trade_ops::sell_value(core, acc, v, a, off).await?;
    let q = super::ride_quote::marginal(&total, &ahead, qty, off);
    let x = super::trade_ops::net_of(core, v, a, &q, qty)
        .await
        .map_err(|e| e.message)?;
    Ok((x, note))
}
