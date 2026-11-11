//! Файл: report/snap.rs
//! Что делает: решение 257 (Тигран 11.11 п. 10) — данные листа «Слепок по балансам» отчёта «Итоги сделок»: в момент
//! выгрузки балансы всех площадок в USDT — каждая биржа (все счета, как панель «Балансы», решение 144: свежий запрос
//! core::venue_balances) и каждый кошелёк DEX (BNB и USDT, core::wallet_balances). Цена — только та, которой бот сам
//! оценивает позицию (решение 152, core/daily_value.rs → trade_ops::sell_value): сколько USDT чистыми получим, если
//! продать всё количество сейчас на площадке, где монета лежит (стакан биржи / котировка DEX кошельком, минус комиссии
//! вне цены); цена за монету = эта сумма / количество. Котируемая монета площадки (venue.quote_asset) — 1. Нет цены —
//! причина текстом («нет цены: …»), а не 0. Активные (открытые) сделки — строкой у площадки, где сейчас их монета или
//! деньги (Deal.at; у DEX — кошелёк сделки): id, монета, направление, шаг, вложено.
//! Одна функция для кнопки интерфейса и Telegram (report/mod.rs results_file).
//! Кто вызывает: report/mod.rs (results_file).
//! От чего зависит: core (venue_balances, wallet_balances, cex_ids, dex_id, deals), core/trade_ops.rs (sell_value),
//! core/balance_sell_check.rs (quote_of, asset_of), core/deal_text.rs (step_name), settings (кошельки), notify/human.rs.
//! Настройки: нет своих (venue.quote_asset — читается).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::Core;
use crate::core::balance_sell_check::{asset_of, quote_of};
use crate::core::deal::Deal;
use crate::core::types::{Asset, DexAccountId, VenueId};
use crate::notify::human::{money, qty as q, venue as name};

use super::rows::f;

/// Строка баланса: счёт, монета, количество, цена за монету в USDT (Err — почему нет цены).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub account: String,
    pub asset: String,
    pub qty: f64,
    pub price: Result<f64, String>,
}

/// Площадка (биржа или кошелёк DEX): имя, когда получен ответ, сбой, строки, активные сделки (текст).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Place {
    pub name: String,
    pub at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub lines: Vec<Line>,
    pub deals: Vec<String>,
}

/// Цена за монету: продажа всего количества сейчас (решение 152), / количество. Ошибок нет: Err — причина.
async fn price(
    core: &Core,
    acc: Option<&DexAccountId>,
    v: &VenueId,
    a: &Asset,
    x: Decimal,
) -> Result<f64, String> {
    let (net, _) = crate::core::trade_ops::sell_value(core, acc, v, a, x).await?;
    Ok(f(net / x))
}

/// Строка активной сделки. Ошибок нет.
fn deal_line(d: &Deal) -> String {
    format!(
        "На момент отчёта активная сделка {}: {} {} → {}, шаг «{}», размером {} USDT (монеты: {} {} на {})",
        d.id,
        d.direction.asset.ticker,
        name(&d.direction.buy_on.0),
        name(&d.direction.sell_on.0),
        crate::core::deal_text::step_name(d.step),
        money(d.invested),
        q(d.qty),
        d.direction.asset.ticker,
        name(&d.at.0)
    )
}

/// Слепок: биржи, затем кошельки DEX. Принимает: ядро. Возвращает: площадки. Ошибок нет: сбой — в поле площадки.
pub async fn gather(core: &Core) -> Vec<Place> {
    let open: Vec<Deal> = core.deals().into_iter().filter(Deal::is_open).collect();
    let dex = core.dex_id();
    let mut out = Vec::new();
    let mut cex = core.cex_ids();
    cex.sort_by(|a, b| a.0.cmp(&b.0));
    for v in &cex {
        let mut p = Place {
            name: name(&v.0),
            deals: open.iter().filter(|d| &d.at == v).map(deal_line).collect(),
            ..Place::default()
        };
        match core.venue_balances(v, None).await {
            Err(e) => p.error = Some(e),
            Ok(b) => {
                p.at = Some(b.at);
                p.error = b.error.clone();
                let quote = quote_of(core, v);
                for a in &b.accounts {
                    if let Some(e) = &a.error {
                        let t = format!("счёт «{}» не прочитан: {e}", a.account);
                        p.error = Some(p.error.take().map_or(t.clone(), |x| format!("{x}; {t}")));
                    }
                    for r in a.rows.iter().filter(|r| !(r.free + r.locked).is_zero()) {
                        let x = r.free + r.locked;
                        let pr = if quote.as_deref() == Ok(r.asset.as_str()) {
                            Ok(1.0)
                        } else {
                            price(core, None, v, &asset_of(core, v, &r.asset), x).await
                        };
                        p.lines.push(Line {
                            account: a.account.clone(),
                            asset: r.asset.clone(),
                            qty: f(x),
                            price: pr,
                        });
                    }
                }
            }
        }
        out.push(p);
    }
    let mut wallets = core.inner.deps.settings.accounts().wallets();
    wallets.sort_by(|a, b| a.name.cmp(&b.name));
    for w in &wallets {
        let acc = DexAccountId(w.id.clone());
        let mut p = Place {
            name: format!("Кошелёк DEX «{}»", w.name),
            deals: open
                .iter()
                .filter(|d| dex.as_ref() == Some(&d.at) && d.dex_account.as_ref() == Some(&acc))
                .map(deal_line)
                .collect(),
            ..Place::default()
        };
        match (core.wallet_balances(&w.id, None).await, &dex) {
            (Err(e), _) => p.error = Some(e),
            (Ok(_), None) => p.error = Some("площадка DEX не подключена".into()),
            (Ok(b), Some(dv)) => {
                p.at = Some(b.at);
                p.error = b.error.clone();
                if let Some(x) = b.bnb.filter(|x| !x.is_zero()) {
                    let t = crate::core::convert::GAS_TICKER;
                    let a = Asset {
                        ticker: t.into(),
                        bsc_contract: None,
                    };
                    let pr = price(core, Some(&acc), dv, &a, x).await;
                    p.lines.push(Line {
                        account: "кошелёк".into(),
                        asset: t.into(),
                        qty: f(x),
                        price: pr,
                    });
                }
                if let (Some(x), Some(t)) = (b.usdt.filter(|x| !x.is_zero()), &b.quote) {
                    p.lines.push(Line {
                        account: "кошелёк".into(),
                        asset: t.clone(),
                        qty: f(x),
                        price: Ok(1.0),
                    });
                }
            }
        }
        out.push(p);
    }
    // Открытая сделка на DEX без кошелька в учёте (до решения 132) — отдельной строкой, чтобы не потерялась.
    let lost: Vec<String> = open
        .iter()
        .filter(|d| dex.as_ref() == Some(&d.at) && d.dex_account.is_none())
        .map(deal_line)
        .collect();
    if !lost.is_empty() {
        out.push(Place {
            name: "DEX (кошелёк сделки не указан)".into(),
            deals: lost,
            ..Place::default()
        });
    }
    out
}
