//! Файл: core/balance_sell_check.rs
//! Что делает: решение 174 — доступна ли кнопка «Продать» у строки панели «Балансы» (монета на счёте биржи) и почему
//! нет. Проверки по порядку: котируемая монета биржи (venue.quote_asset) — кнопки нет; свободных нет; монета уже
//! продаётся кнопкой; монета в сделке бота (метка Я-2 в active, решение 118) или в незавершённой сделке на этой бирже
//! (застрявшая, остановленная, ждёт решения) — «используйте „Ручное управление“»; счёт не торговый и перевода с него
//! в адаптере нет (контракт account_role); нет пары / стакана / глубины;
//! биржа не примет ордер — та же проверка, что у предпроверки продажи (контракт check_order → order_rules::ready
//! адаптера, решение 134). Те же проверки ядро повторяет при нажатии (balance_sell.rs).
//! Кто вызывает: core/balance_sell.rs (нажатие), api/balances.rs (состояние кнопок в ответе панели).
//! От чего зависит: engine.rs (рынок, сделки, метки), coin_key.rs, worst.rs, deal.rs, gateway (контракт Venue), settings.
//! Настройки: venue.quote_asset (по бирже; читается, своих нет).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Side, TradeRequest, VenueId};
use crate::gateway::contract::{AccountRole, Venue};
use crate::settings::read::At;

use super::coin_key::CoinKey;
use super::deal::Step;
use super::engine::{Core, lk, rd};

/// Отказ: монета в сделке бота.
pub const IN_DEAL: &str = "монета в сделке — используйте «Ручное управление»";
/// Отказ: монета уже продаётся этой кнопкой.
pub const SELLING: &str = "уже продаётся";

/// Область настроек биржи. Ошибок нет.
fn at(v: &VenueId) -> At<'_> {
    At {
        venue: Some(&v.0),
        coin: None,
    }
}

/// Котируемая монета биржи (venue.quote_asset). Ошибка: настройка не задана (текст хранилища настроек).
pub fn quote_of(core: &Core, v: &VenueId) -> Result<String, String> {
    core.inner
        .deps
        .settings
        .text("venue.quote_asset", at(v))
        .map_err(|e| format!("{}: {}", v.0, e.message))
}

/// Монета по тикеру биржи: из справочника монет (с адресом контракта — ключ метки Я-2), иначе — тикер без адреса.
/// Принимает: ядро, биржу, тикер. Возвращает: монету. Ошибок нет.
pub fn asset_of(core: &Core, v: &VenueId, ticker: &str) -> Asset {
    rd(&core.inner.tickers).get(v, ticker).unwrap_or(Asset {
        ticker: ticker.to_string(),
        bsc_contract: None,
    })
}

/// Почему монету сейчас нельзя продать кнопкой. Принимает: ядро, биржу, монету.
/// Возвращает: None — свободна; Some(SELLING | IN_DEAL). Ошибок нет.
pub fn busy(core: &Core, v: &VenueId, a: &Asset) -> Option<&'static str> {
    let key = CoinKey::of(a);
    if lk(&core.inner.balance_sells).contains(&key) {
        return Some(SELLING);
    }
    if lk(&core.inner.active).contains(&key) {
        return Some(IN_DEAL);
    }
    // Незавершённая сделка этой монеты на этой бирже: идёт, застряла, ждёт решения или остановлена с монетами.
    let open = lk(&core.inner.deals).values().any(|d| {
        let live = match d.step {
            Step::Closed | Step::TakenOver => false,
            Step::Halted => d.qty > Decimal::ZERO,
            _ => true,
        };
        live && d.at == *v
            && (CoinKey::of(&d.direction.asset) == key || d.direction.asset.ticker == a.ticker)
    });
    open.then_some(IN_DEAL)
}

/// Нужен ли перевод на торговый счёт перед продажей. Принимает: адаптер, счёт (название из account_balances).
/// Возвращает: false — счёт торговый; true — перевести (to_trading.rs, контракт move_to_trading).
/// Ошибки: перевода с этого счёта в адаптере нет.
pub fn needs_move(gw: &dyn Venue, account: &str) -> Result<bool, String> {
    match gw.account_role(account) {
        AccountRole::Trading => Ok(false),
        AccountRole::ToTrading => Ok(true),
        AccountRole::Other => Err(format!(
            "перевода со счёта «{account}» на торговый в адаптере {} нет — переведите вручную",
            gw.id().0
        )),
    }
}

/// Ордер продажи — как его поставит исполнение (trade_ops::trade): худшая цена — самый глубокий нужный уровень стакана
/// (worst.rs). Принимает: ядро, биржу, монету, количество, свой ID. Возвращает: запрос.
/// Ошибки: нет пары, нет стакана (монета не в подписке бота), глубины не хватает.
pub fn request(
    core: &Core,
    v: &VenueId,
    a: &Asset,
    qty: Decimal,
    id: &str,
) -> Result<TradeRequest, String> {
    let quote = quote_of(core, v)?;
    let worst = {
        let m = rd(&core.inner.market);
        if m.book(v, &a.ticker).is_none() {
            return Err(if m.pair(v, &a.ticker).is_none() {
                format!(
                    "нет пары {}/{quote} на {} (или монета не в подписке бота)",
                    a.ticker, v.0
                )
            } else {
                format!(
                    "нет стакана {}/{quote} на {}: монета не в подписке бота — продать по рынку нельзя",
                    a.ticker, v.0
                )
            });
        }
        m.deepest_price(v, &a.ticker, Side::Sell, qty)
            .ok_or_else(|| format!("глубины стакана {}/{quote} не хватает на {qty}", a.ticker))?
    };
    Ok(TradeRequest {
        venue: v.clone(),
        asset: a.clone(),
        side: Side::Sell,
        qty,
        worst_price: worst,
        deal_id: id.to_string(),
        route: None,
        // Решение 177: продаётся монета, которая уже есть, — продажа выхода (как у trade_ops.rs).
        exit_position: true,
    })
}

/// Состояние кнопки «Продать» строки панели. Принимает: ядро, адаптер биржи, счёт, тикер, свободно на счёте.
/// Возвращает: None — кнопки нет (котируемая монета); Some(Ok) — доступна; Some(Err(причина)) — недоступна.
pub async fn state(
    core: &Core,
    gw: &dyn Venue,
    account: &str,
    ticker: &str,
    free: Decimal,
) -> Option<Result<(), String>> {
    let v = gw.id();
    match quote_of(core, &v) {
        Ok(q) if q == ticker => None,
        Ok(_) => Some(check(core, gw, account, ticker, free).await),
        Err(e) => Some(Err(e)),
    }
}

/// Все проверки, кроме котируемой монеты (см. state). Ошибки: причина недоступности.
async fn check(
    core: &Core,
    gw: &dyn Venue,
    account: &str,
    ticker: &str,
    free: Decimal,
) -> Result<(), String> {
    let v = gw.id();
    if free <= Decimal::ZERO {
        return Err("свободных нет".into());
    }
    let a = asset_of(core, &v, ticker);
    if let Some(w) = busy(core, &v, &a) {
        return Err(w.into());
    }
    needs_move(gw, account)?;
    let req = request(core, &v, &a, free, "check")?;
    gw.check_order(&req)
        .await
        .map_err(|e| format!("биржа не примет ордер: {}", e.message))
}
