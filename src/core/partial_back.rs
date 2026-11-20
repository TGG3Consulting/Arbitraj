//! Файл: core/partial_back.rs
//! Что делает: решение 480 (ревью) — «продать на A» частичной покупки (partial.rs) с тем же худшим случаем, что у продажи
//! на B: сдвиг площадки A одним чтением (ride_shift::read для направления с площадкой продажи A) — наши идущие сделки
//! групп, которые ещё продадут на A (решение 464, правило Тиграна 20.11: свои идущие сделки через пул / стакан
//! учитываются), и остаток наших продаж на A с неизвестным исходом (решение 480). Сдвиг 0 — котировка как раньше
//! (trade_ops::sell_value). Иначе — пара котировок ride_pair::exec_pair (биржа — один снимок стакана), наша часть —
//! ride_quote::marginal, чистыми — trade_ops::net_of; глубины на сдвиг + объём нет — «продать на A» не считается.
//! Ревью 3: сдвиг читается первым (ride_shift::read — со счётчиком итогов продаж, его сверяет after); при исполнении
//! и сдвиге 0 — без котировки; котировки DEX при исполнении — с id сделки (t78).
//! Ревью 2: та же защита свежести, что у ride_quote::live_sell — граница своей продажи (ride_settled::in_flight / after:
//! ордер в полёте — ждём его итога, кончился за время котировки — считаем снова) и след продаж (ride_trail::check).
//! Тот же худший случай — у границы «итог ≥ 0» при исполнении продажи на A (exec_values → sell::market_guarded_back).
//! Кто вызывает: core/partial.rs (value), core/sell.rs (exec_values).
//! От чего зависит: ride_shift.rs, ride_pair.rs, ride_quote.rs (marginal), ride_settled.rs, ride_trail.rs, sale_marks.rs,
//! trade_ops.rs, coin_key.rs, core/types.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, Direction, Quote};

use super::coin_key::CoinKey;
use super::deal::Deal;
use super::engine::Core;

/// Котировка продажи на A на сдвиге: (наша часть, сдвиг, строка худшего случая).
type Shifted = (Quote, Decimal, Option<String>);

/// Что делает: котировка продажи всего количества сделки на A после наших продаж впереди (шапка), с ожиданием итога
/// своего ордера продажи на A. Принимает: ядро, счёт DEX (None — биржа), сделку, id сделки для исполнения (Some —
/// продажа этой сделки: налоги кошелька сделки, t78; при сдвиге 0 — None без котировки; None — оценка для выбора).
/// Возвращает: Shifted; None — только при исполнении и сдвиге 0. Ошибки: текст — нет котировки или глубины, данные не
/// свежее продажи сделки группы, запись сделки с неизвестным исходом не найдена.
async fn quote(
    core: &Core,
    acc: Option<&DexAccountId>,
    deal: &Deal,
    exec: Option<&str>,
) -> Result<Option<Shifted>, String> {
    let (v, a) = (&deal.direction.buy_on, &deal.direction.asset);
    let back = Direction {
        sell_on: v.clone(),
        ..deal.direction.clone()
    };
    loop {
        // Будильник — до котировки: конец нашего ордера во время котировки не теряется (как partial.rs).
        let wake = core.inner.sales.done.notified();
        let r = once(core, acc, deal, &back, exec).await;
        if !matches!(&r, Err(e) if e.contains(super::sale_marks::MARK)) {
            return r;
        }
        if super::sale_marks::busy(core, v, a) {
            wake.await;
        }
    }
}

/// Одна попытка quote (без ожидания). Сдвиг — первым (ride_shift::read: счётчик итогов продаж и количества одним
/// чтением, ревью 3); при исполнении и сдвиге 0 — None сразу, без котировки. Ошибки: как у quote; с меткой MARK —
/// своя продажа на A в полёте или кончилась после чтения сдвига.
async fn once(
    core: &Core,
    acc: Option<&DexAccountId>,
    deal: &Deal,
    back: &Direction,
    exec: Option<&str>,
) -> Result<Option<Shifted>, String> {
    let (v, a, qty) = (&back.sell_on, &back.asset, deal.qty);
    let sh = super::ride_shift::read(core, back)?;
    if exec.is_some() && sh.off.is_zero() {
        return Ok(None);
    }
    if !sh.live {
        // Групп нет и продаж с неизвестным исходом на A нет — как до решения 480.
        let q = super::trade_ops::sell_value(core, acc, v, a, qty).await?.1;
        return Ok(Some((q, sh.off, sh.note)));
    }
    super::ride_settled::in_flight(core, back)?;
    let q = if sh.off.is_zero() {
        super::trade_ops::sell_value(core, acc, v, a, qty).await?.1
    } else {
        let pair = super::ride_pair::exec_pair(core, acc, exec, v, a, qty, sh.off);
        let (t, h) = pair.await?;
        if t.fillable_qty < qty + sh.off {
            return Err(format!(
                "{}: глубины хватает на {} из {} (наши продажи впереди {} + объём {qty})",
                v.0,
                t.fillable_qty,
                qty + sh.off,
                sh.off
            ));
        }
        super::ride_quote::marginal(&t, &h, qty, sh.off)
    };
    super::ride_settled::after(core, back, sh.seq)?;
    super::ride_trail::check(core, &CoinKey::of(a), v, &q)?;
    Ok(Some((q, sh.off, sh.note)))
}

/// Что делает: чистыми (USDT) от продажи всего количества сделки на A сейчас, после наших продаж впереди — для выбора
/// partial::choose. Принимает: ядро, счёт DEX (None — биржа), сделку. Возвращает: (сумма, строка хронологии о худшем
/// случае; None — продаж с неизвестным исходом на A нет). Ошибки: как у quote; нет курса затрат.
pub async fn value(
    core: &Core,
    acc: Option<&DexAccountId>,
    deal: &Deal,
) -> Result<(Decimal, Option<String>), String> {
    let Some((q, _, note)) = quote(core, acc, deal, None).await? else {
        return Err("котировки продажи на A нет".into());
    };
    let (v, a) = (&deal.direction.buy_on, &deal.direction.asset);
    let x = super::trade_ops::net_of(core, v, a, &q, deal.qty)
        .await
        .map_err(|e| e.message)?;
    Ok((x, note))
}

/// Что делает: ревью 2, п. 4 — оценка продажи на A при исполнении на том же худшем случае, что у решения: (чистыми,
/// выход до затрат = количество × средняя) для границы «итог ≥ 0» (remeasure::of). Котировки DEX — как у настоящей
/// продажи этой сделки (id сделки, t78). Принимает: ядро, сделку (на A). Возвращает: None — сдвига нет (без котировки;
/// граница — по котировке продажи, как раньше). Ошибки: как у value.
pub async fn exec_values(core: &Core, deal: &Deal) -> Result<Option<(Decimal, Decimal)>, String> {
    let acc = super::dex_account::of(core, deal)?;
    let Some((q, _, _)) = quote(core, acc.as_ref(), deal, Some(&deal.id)).await? else {
        return Ok(None);
    };
    let (v, a) = (&deal.direction.buy_on, &deal.direction.asset);
    let x = super::trade_ops::net_of(core, v, a, &q, deal.qty)
        .await
        .map_err(|e| e.message)?;
    Ok(Some((x, deal.qty * q.avg_price)))
}
