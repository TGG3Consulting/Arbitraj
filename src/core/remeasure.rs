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
//! t81 (дополнение к 247 п. Г, Тигран 10.11: согласовано): у путей, где пере-замер добавил t78 (первая продажа сделки,
//! «Вернуть боту», частичная покупка, «Ожидание» по истечении stuck.wait_ms), — порог PROFITABLE_OPEN: та же граница, и
//! признак Remeasure.any_executor — адаптер DEX может исполнить продажу и агрегатором (без пере-замера), если так
//! настроен и его маршрут лучше; у остальных путей с пере-замером признака нет — только свой сборщик (п. Г, как было).
//! Кто вызывает: core/sell.rs (market_guarded; sell_or_stuck — PROFITABLE_OPEN, sell_after_relay — PROFITABLE),
//! core/stuck.rs, core/relay_here.rs, core/executor.rs (частичная покупка — PROFITABLE_OPEN).
//! От чего зависит: deal.rs, credit_shortfall.rs (shortfall_pct), types (Remeasure).
//! Настройки: нет (пороги — у вызывающих: stuck.max_loss_pct, stuck.relay_max_loss_pct).

use rust_decimal::Decimal;

use crate::core::types::Remeasure;

use super::deal::Deal;

/// Разрешён ли пере-замер продажи и при каком пороге.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// Пере-замера нет (как до решения 247): продажа по рынку не кнопкой и не автоматикой с порогом (sell::guard_of).
    /// t78: первая продажа сделки, «Вернуть боту», «Ожидание» по истечении stuck.wait_ms, частичная покупка — с порогом
    /// «итог ≥ 0» (t81: PROFITABLE_OPEN).
    Off,
    /// Кнопка человека (решение 235: без порогов убытка) — пере-замер всегда, граница 0.
    Any,
    /// Автоматика: убыток при реальном выходе не больше этого порога, % (stuck.max_loss_pct — «Порог убытка»;
    /// stuck.relay_max_loss_pct — продажа здесь в «Перегоне»; 0 — итог сделки ≥ 0, PROFITABLE: «Ожидание: стало выгодно»,
    /// продажа на цели после автоматического «Перегона»; пути t78 — Guard::Open, t81).
    MaxLoss(Decimal),
    /// t81 (дополнение к 247 п. Г, Тигран 10.11: согласовано): порог как MaxLoss, и продажу можно исполнить любым
    /// исполнителем DEX (Remeasure.any_executor = да): агрегатор, если адаптер так настроен и его маршрут лучше, — без
    /// пере-замера; свой сборщик — с пере-замером по этой границе. Только пути, где пере-замер добавил t78
    /// (PROFITABLE_OPEN): первая продажа сделки, «Вернуть боту», частичная покупка, «Ожидание» по истечении.
    Open(Decimal),
}

/// Порог «выгодно» (решение 47: итог сделки ≥ 0) как порог убытка 0 % — у продажи режима «Ожидание: стало выгодно»
/// (stuck.rs) и у продажи на цели сразу после автоматического «Перегона» (sell::sell_after_relay, дополнение 10.11);
/// пути t78 (первая продажа сделки, «Вернуть боту», частичная покупка, «Ожидание» по истечении) — PROFITABLE_OPEN (t81).
pub const PROFITABLE: Guard = Guard::MaxLoss(Decimal::ZERO);

/// t81 (дополнение к 247 п. Г, Тигран 10.11: согласовано): порог «итог сделки ≥ 0» у путей, где пере-замер добавил t78
/// (первая продажа сделки и «Вернуть боту» — sell::sell_or_stuck, частичная покупка — executor.rs, «Ожидание» по
/// истечении stuck.wait_ms — stuck.rs): граница та же, что у PROFITABLE, и продажа может идти агрегатором (Guard::Open).
pub const PROFITABLE_OPEN: Guard = Guard::Open(Decimal::ZERO);

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
        Guard::MaxLoss(max) | Guard::Open(max) => max,
    };
    if deal.invested.is_zero() {
        return (max >= Decimal::ZERO).then_some(Decimal::ZERO);
    }
    let v_min =
        deal.invested + deal.spent - deal.proceeds - max * deal.invested / Decimal::ONE_HUNDRED;
    Some((v_min + (gross - value)).max(Decimal::ZERO))
}

/// Условия пере-замера для запроса: граница (floor), налог, наблюдённый в сети (tax_seen_pct), и признак «любой
/// исполнитель» (t81: только у Guard::Open). Принимает: как floor. Возвращает: None — пере-замера нет. Ошибок нет.
pub fn of(deal: &Deal, value: Decimal, gross: Decimal, g: Guard) -> Option<Remeasure> {
    floor(deal, value, gross, g).map(|floor| Remeasure {
        floor,
        tax_seen_pct: tax_seen_pct(deal),
        any_executor: matches!(g, Guard::Open(_)),
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
