//! Файл: core/remeasure.rs
//! Что делает: решение 247 (Тигран 10.11: согласовано; живой случай PIG 09.11, сделка 20261109202626272PIG — каждая
//! продажа DEX откатывалась «minOut»: minOut = ожидаемое по модели налога × (1 − допуск), а реальный выход ниже) —
//! граница пере-замера для продажи: наименьший реальный выход продажи R (USDT), при котором адаптер DEX после отката
//! пробы по minOut повторяет продажу с minOut от R (TradeRequest.remeasure). Граница — тот же порог убытка, по
//! которому ядро решило продать: убыток deal.loss_pct_if(R − затраты вне цены) ≤ порога. Новых настроек нет.
//! Налог, наблюдённый в сети (новые факты PIG 09.11: перевод w2 → пул удержал ровно 5.0 %, проба налога дала 4.0003 %,
//! симуляция до покупки — выход 102.16 при реальном ~101.1): tax_seen_pct — недостача зачисления вывода этой сделки
//! (отправлено минус комиссия вывода из ответа биржи, если она в монете; иначе вся недостача) — передаётся адаптеру,
//! граница выхода при пере-замере не выше выхода модели при этом налоге (TradeRequest.remeasure, types/trade.rs).
//! Кто вызывает: core/sell.rs (market_guarded; sell_or_stuck и sell_after_relay — порог PROFITABLE), core/stuck.rs,
//! core/relay_here.rs, core/executor.rs (частичная покупка — PROFITABLE).
//! От чего зависит: deal.rs, credit_shortfall.rs (shortfall_pct), types (Remeasure).
//! Настройки: нет (пороги — у вызывающих: stuck.max_loss_pct, stuck.relay_max_loss_pct).

use rust_decimal::Decimal;

use crate::core::types::Remeasure;

use super::deal::Deal;

/// Разрешён ли пере-замер продажи и при каком пороге.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// Пере-замера нет (как до решения 247): продажа по рынку не кнопкой и не автоматикой с порогом (sell::guard_of).
    /// t78: первая продажа сделки, «Вернуть боту», «Ожидание» по истечении stuck.wait_ms, частичная покупка — PROFITABLE.
    Off,
    /// Кнопка человека (решение 235: без порогов убытка) — пере-замер всегда, граница 0.
    Any,
    /// Автоматика: убыток при реальном выходе не больше этого порога, % (stuck.max_loss_pct — «Порог убытка»;
    /// stuck.relay_max_loss_pct — продажа здесь в «Перегоне»; 0 — итог сделки ≥ 0, PROFITABLE: «Ожидание: стало выгодно»,
    /// продажа на цели после автоматического «Перегона», первая продажа сделки, «Вернуть боту», «Ожидание» по истечении
    /// stuck.wait_ms, продажа после частичной покупки).
    MaxLoss(Decimal),
}

/// Порог «выгодно» (решение 47: итог сделки ≥ 0) как порог убытка 0 % — у продажи режима «Ожидание: стало выгодно»
/// (stuck.rs) и у продажи на цели сразу после автоматического «Перегона» (sell::sell_after_relay, дополнение 10.11);
/// t78 (дополнение 10.11, Тигран: согласовано) — и у первой продажи сделки и «Вернуть боту» (sell::sell_or_stuck), у
/// продажи после частичной покупки (executor.rs) и «Ожидания» по истечении stuck.wait_ms (stuck.rs).
pub const PROFITABLE: Guard = Guard::MaxLoss(Decimal::ZERO);

/// Граница реального выхода продажи всего количества сделки. Принимает: сделку, оценку «продать сейчас» чистыми
/// (trade_ops::deal_quote — котировка × количество минус затраты вне цены), выход котировки до затрат (количество ×
/// средняя котировки), порог. Возвращает: None — пере-замера нет; Some(F) — повтор с minOut от реального выхода R
/// разрешён, если R ≥ F (F ≥ 0). Затраты вне цены (газ и т. п.) считаются теми же, что в котировке: при выходе R
/// чистыми будет R − (gross − value). Ошибок нет.
///
/// Вывод: loss_pct_if(v) ≤ max ⇔ v ≥ вложено + потрачено − выручка − max × вложено / 100 = v_min (deal.rs); значит
/// R ≥ v_min + (gross − value). Вложено 0 — loss_pct_if всегда 0 (deal.rs): порог 0 и выше — граница 0, ниже — нет.
pub fn floor(deal: &Deal, value: Decimal, gross: Decimal, g: Guard) -> Option<Decimal> {
    let max = match g {
        Guard::Off => return None,
        Guard::Any => return Some(Decimal::ZERO),
        Guard::MaxLoss(max) => max,
    };
    if deal.invested.is_zero() {
        return (max >= Decimal::ZERO).then_some(Decimal::ZERO);
    }
    let v_min =
        deal.invested + deal.spent - deal.proceeds - max * deal.invested / Decimal::ONE_HUNDRED;
    Some((v_min + (gross - value)).max(Decimal::ZERO))
}

/// Условия пере-замера для запроса: граница (floor) и налог, наблюдённый в сети (tax_seen_pct). Принимает: как floor.
/// Возвращает: None — пере-замера нет. Ошибок нет.
pub fn of(deal: &Deal, value: Decimal, gross: Decimal, g: Guard) -> Option<Remeasure> {
    floor(deal, value, gross, g).map(|floor| Remeasure {
        floor,
        tax_seen_pct: tax_seen_pct(deal),
    })
}

/// Налог перевода, наблюдённый в сети в этой сделке, %: последний перевод монеты сделки на площадку, где она сейчас
/// (deal.transfers — парами «вывод, зачисление», fills.rs): ожидалось = отправлено − комиссия вывода из ответа
/// площадки (Transfer.fee, только если она в этой монете; иначе не вычитается — вся недостача считается налогом, в
/// безопасную сторону), недостача зачисления от ожидаемого (credit_shortfall::shortfall_pct). None — перевода нет или
/// недостачи нет. Ошибок нет.
pub fn tax_seen_pct(deal: &Deal) -> Option<Decimal> {
    let coin = &deal.direction.asset.ticker;
    let pair = deal.transfers.chunks_exact(2).rev().find(|p| {
        p[1].venue == deal.at && &p[0].asset.ticker == coin && p[0].venue != p[1].venue
    })?;
    let (sent, got) = (&pair[0], &pair[1]);
    let fee = sent
        .fee
        .as_ref()
        .filter(|f| &f.asset == coin)
        .map_or(Decimal::ZERO, |f| f.amount);
    super::credit_shortfall::shortfall_pct(sent.amount - fee, got.amount)
}
