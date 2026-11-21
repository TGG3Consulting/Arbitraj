//! Файл: core/manual_sold_check.rs
//! Что делает: решение 236, дополнение п. 3 (техлид 09.11 по поручению Тиграна) — проверка ввода «Продано вручную»:
//! введённая выручка человека больше стоимости остатка сделки по рынку больше чем на manual.sold_max_over_pct % —
//! отказ с причиной и просьбой проверить (живой случай 20261108142947740PIT: за остаток 729 PIT, стоивший ~0, введено
//! 44.88 — выручка всей сделки). Стоимость по рынку — текущая котировка продажи остатка там, где он лежит, кошельком
//! сделки (trade_ops::deal_value — та же котировка, что у «Продать сейчас»): количество × средняя цена котировки (до
//! затрат продажи — человек вводит полученное, его затраты могли быть любыми, верхняя граница честная).
//! Настройка не задана — проверки нет (как раньше). Нет котировки (нет стакана, глубины, кошелька) — проверки нет,
//! пометка в ответе и хронологии. Остатка нет (qty = 0) — проверять нечего.
//! Кто вызывает: core/manual_halted.rs (sold_manually).
//! От чего зависит: trade_ops.rs (deal_value), deal.rs, settings.
//! Настройки: manual.sold_max_over_pct (необязательная).

use rust_decimal::Decimal;

use crate::settings::read::At;

use super::deal::Deal;
use super::engine::Core;

/// Что делает: проверка ввода. Принимает: ядро, сделку (снимок до закрытия), введённую выручку человека.
/// Возвращает: Ok(None) — проверки нет (настройка не задана, остатка нет); Ok(Some(строка)) — проверено или котировки
/// нет (строка — для ответа и хронологии). Ошибки: текст отказа — ввод больше стоимости по рынку больше чем на N %.
pub async fn check(core: &Core, deal: &Deal, got: Decimal) -> Result<Option<String>, String> {
    let Ok(pct) = core
        .inner
        .deps
        .settings
        .decimal("manual.sold_max_over_pct", At::default())
    else {
        return Ok(None);
    };
    if deal.qty <= Decimal::ZERO {
        return Ok(None);
    }
    let (ticker, qty, at) = (&deal.direction.asset.ticker, deal.qty, &deal.at.0);
    let value = match super::trade_ops::deal_value(core, deal).await {
        Ok((_, q)) => qty * q.avg_price,
        Err(e) => {
            return Ok(Some(format!(
                "проверка ввода (manual.sold_max_over_pct) не выполнена — нет котировки продажи {qty} {ticker} на {at}: {e}"
            )));
        }
    };
    let max = value * (Decimal::ONE + pct / Decimal::ONE_HUNDRED);
    if got > max {
        return Err(format!(
            "введено {got} USDT за {qty} {ticker}, а по рынку остаток стоит {} USDT (котировка продажи на {at}) — больше чем на {pct} % (manual.sold_max_over_pct). Проверьте сумму: только ваша выручка за эти монеты, без выручки бота. Если сумма верна (цена выросла), повторите позже или временно очистите manual.sold_max_over_pct",
            value.round_dp(4)
        ));
    }
    Ok(Some(format!(
        "проверка ввода: остаток по рынку {} USDT, ввод {got} USDT — в пределах {pct} %",
        value.round_dp(4)
    )))
}
