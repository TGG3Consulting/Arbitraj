//! Файл: core/balance_sell.rs
//! Что делает: решение 174 (Тигран, 05.11.2026) — кнопка «Продать» у монеты на счёте биржи в панели «Балансы»: всё
//! свободное количество монеты — по рынку в котируемую монету (venue.quote_asset, USDT); выручка остаётся на торговом
//! (спот) счёте. Не вторая логика продажи: тот же ордер, что у «Продать сейчас» — trade_ops::trade (худшая цена —
//! самый глубокий нужный уровень стакана, worst.rs; ордер адаптера — order_rules::ready и venue.order_time_in_force,
//! IOC), перед ним — та же проверка check_order (решение 134). Монета не на торговом счёте — сначала перевод на него
//! общим шагом to_trading.rs (контракт move_to_trading; роль счёта — account_role; у биржи перевода нет — отказ).
//! Безопасность: монета в сделке бота (метка Я-2, незавершённая сделка на бирже) — отказ «используйте „Ручное
//! управление“»; на время продажи — метка монеты в active (бот по ней сделку не начнёт) и в balance_sells (повторное
//! нажатие — «уже продаётся»); метки снимаются при любом выходе, в том числе при ошибке и панике (Mark, урок решения 165).
//! Итог — текст для панели, журнал core.balance_sell с тем, кто нажал, событие Telegram balance.sell. В учёт сделок и
//! итог дня выручка НЕ идёт: это не сделка бота.
//! Кто вызывает: core/commands.rs (ManualAction::SellBalance — POST /api/manual/command из панели «Балансы»).
//! От чего зависит: balance_sell_check.rs (проверки), to_trading.rs (перевод), trade_ops.rs (ордер), engine.rs (метки), gateway (контракт Venue),
//! balance_sell_report.rs (итог: текст, журнал, Telegram).
//! Настройки: свои — нет; читаются venue.quote_asset (balance_sell_check.rs) и настройки ордера
//! адаптера (venue.order_time_in_force и др.).

use rust_decimal::Decimal;

use crate::core::types::{Asset, ManualAction, Side, VenueId};
use crate::gateway::contract::{Venue, VenueKind};

use super::balance_sell_check as chk;
use super::coin_key::CoinKey;
use super::engine::{Core, lk};

/// Метка продажи монеты: снимается при выходе из любой ветки (и при панике задачи).
struct Mark {
    core: Core,
    key: CoinKey,
}

impl Drop for Mark {
    fn drop(&mut self) {
        lk(&self.core.inner.active).remove(&self.key);
        lk(&self.core.inner.balance_sells).remove(&self.key);
    }
}

/// Ставит метку монеты. Принимает: ядро, биржу, монету. Возвращает: метку. Ошибки: «уже продаётся», «в сделке».
fn mark(core: &Core, v: &VenueId, a: &Asset) -> Result<Mark, String> {
    if let Some(w) = chk::busy(core, v, a) {
        return Err(w.into());
    }
    let key = CoinKey::of(a);
    if !lk(&core.inner.balance_sells).insert(key.clone()) {
        return Err(chk::SELLING.into());
    }
    if !lk(&core.inner.active).insert(key.clone()) {
        lk(&core.inner.balance_sells).remove(&key);
        return Err(chk::IN_DEAL.into());
    }
    Ok(Mark {
        core: core.clone(),
        key,
    })
}

/// Команда «Продать» панели «Балансы». Принимает: ядро, действие, кто (commands::who). Возвращает: None — действие не
/// отсюда; Some(итог): Ok(«Продано X …, получено Y USDT») или Err(причина отказа).
pub async fn command(core: &Core, a: &ManualAction, who: &str) -> Option<Result<String, String>> {
    let ManualAction::SellBalance {
        venue,
        account,
        asset,
    } = a
    else {
        return None;
    };
    let v = VenueId(venue.clone());
    let res = sell(core, &v, account, asset).await;
    super::balance_sell_report::report(core, &v, account, asset, who, &res).await;
    Some(res)
}

/// Продажа под меткой. Ошибки: текст причины (уже с биржей и счётом, где нужно).
async fn sell(core: &Core, v: &VenueId, account: &str, ticker: &str) -> Result<String, String> {
    let gw = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    if gw.kind() != VenueKind::Cex {
        return Err(format!("{} — не биржа", v.0));
    }
    let quote = chk::quote_of(core, v)?;
    if ticker == quote {
        return Err(format!(
            "{ticker} — котируемая монета {}, продавать нечего",
            v.0
        ));
    }
    let a = chk::asset_of(core, v, ticker);
    let _mark = mark(core, v, &a)?;
    let free = row_free(gw.as_ref(), account, ticker).await?;
    let moved = if chk::needs_move(gw.as_ref(), account)? {
        // Решение 211: площадке — её тикер монеты.
        let on = super::venue_ticker::on_venue(core, &a, v);
        Some(super::to_trading::move_from(gw.as_ref(), account, &on, free).await?)
    } else {
        None
    };
    let after = |e: String| match &moved {
        Some(m) => format!("{m}; продажа не прошла: {e}"),
        None => e,
    };
    let on = gw
        .balance(ticker)
        .await
        .map_err(|e| after(format!("{}: баланс торгового счёта: {}", v.0, e.message)))?
        .map_or(Decimal::ZERO, |b| b.free);
    // С фонда продаётся переведённое (не больше пришедшего на торговый); с торгового — всё свободное там сейчас.
    let qty = if moved.is_some() { free.min(on) } else { on };
    if qty <= Decimal::ZERO {
        return Err(after(format!(
            "на торговом счёте {} нет свободных {ticker} — если перевод ещё идёт, нажмите «Продать» у торгового счёта позже",
            v.0
        )));
    }
    let id = format!(
        "bal{}{ticker}",
        chrono::Utc::now()
            .timestamp_millis()
            .rem_euclid(1_000_000_000)
    );
    let req = chk::request(core, v, &a, qty, &id).map_err(after)?;
    gw.check_order(&req)
        .await
        .map_err(|e| after(format!("биржа не примет ордер: {}", e.message)))?;
    let r = super::trade_ops::trade(
        core,
        &id,
        v,
        &a,
        Side::Sell,
        qty,
        None,
        req.worst_price,
        None,
        // Не сделка — газ впустую только в статистику монеты (gas_stats), как раньше.
        None,
    )
    .await
    .map_err(|e| {
        after(if e.outcome_unknown() {
            format!("исход ордера неизвестен ({e}) — проверьте на бирже")
        } else {
            e.message
        })
    })?;
    Ok(super::balance_sell_report::sold_text(
        &r,
        qty,
        &quote,
        moved.as_deref(),
    ))
}

/// Свободно монеты на счёте — свежим запросом account_balances. Ошибки: связь, счёт не прочитан, монеты нет.
async fn row_free(gw: &dyn Venue, account: &str, ticker: &str) -> Result<Decimal, String> {
    let v = gw.id();
    let accs = gw
        .account_balances()
        .await
        .map_err(|e| format!("{}: балансы: {}", v.0, e.message))?;
    let acc = accs
        .iter()
        .find(|x| x.account == account)
        .ok_or_else(|| format!("{}: счёта «{account}» нет", v.0))?;
    if let Some(e) = &acc.error {
        return Err(format!("{}: счёт «{account}» не прочитан: {e}", v.0));
    }
    let free = acc
        .balances
        .iter()
        .find(|b| b.asset == ticker)
        .map_or(Decimal::ZERO, |b| b.free);
    if free <= Decimal::ZERO {
        return Err(format!(
            "на счёте «{account}» {} нет свободных {ticker}",
            v.0
        ));
    }
    Ok(free)
}
