//! Файл: notify/ctl/balances.rs
//! Что делает: экраны «Балансы» чата «Управление» (решение 176) — список бирж и кошельков DEX; по бирже — все счета
//! (спот, фонд, фьючерсы… — как панель «Балансы», решение 144) с USDT и монетами, у монеты — кнопка «Продать»
//! (решение 174: та же доступность и причина, что в панели; недоступная — с причиной в тексте и по нажатию); по
//! кошельку — BNB и USDT. Только рендер из памяти панели (core/balances_cache.rs) — запросов к биржам здесь нет.
//! Кто вызывает: notify/ctl/press.rs, notify/ctl/live.rs.
//! От чего зависит: core/balances_view.rs (VenueView, WalletView), ctl/kb.rs, ctl/sessions.rs (Ref), notify/human.rs.
//! Настройки: нет.

use crate::core::balances_view::{VenueView, WalletView};
use crate::core::types::VenueId;
use crate::settings::accounts::Wallet;

use super::super::human::{qty, venue as name};
use super::kb::{Screen, btn};
use super::sessions::Ref;

/// Список: биржи и кошельки DEX. Принимает: биржи, кошельки. Ошибок нет.
pub fn list(cex: &[VenueId], wallets: &[Wallet]) -> Screen {
    let mut s = Screen::new("Балансы — выберите биржу или кошелёк DEX.");
    for v in cex {
        s = s.one(btn(&name(&v.0), &format!("k:bv.{}", v.0)));
    }
    for w in wallets {
        s = s.one(btn(
            &format!("Кошелёк DEX «{}»", w.name),
            &format!("k:bw.{}", w.id),
        ));
    }
    if cex.is_empty() && wallets.is_empty() {
        s = s.line("Бирж и кошельков DEX нет.");
    }
    s.nav(Some("k:root"))
}

/// Биржа: счета, строки и кнопки «Продать» (callback «s:<номер отрисовки>.<строка>»).
/// Принимает: ответ биржи, номер отрисовки. Возвращает: (экран, денежные кнопки по порядку). Ошибок нет.
pub fn venue(b: &VenueView, sid: u64) -> (Screen, Vec<Ref>) {
    let mut s = Screen::new(format!(
        "{} — балансы на {}",
        name(&b.venue),
        b.at.format("%H:%M:%S UTC")
    ));
    let mut refs = Vec::new();
    if let Some(e) = &b.error {
        s = s.line(format!("Не получено: {e}"));
    }
    for a in &b.accounts {
        let role = if a.trading { " (торговый)" } else { "" };
        s = s.line(format!("\n{}{role}", a.account));
        if let Some(e) = &a.error {
            s = s.line(format!("  не прочитан: {e}"));
        }
        if let Some(n) = &a.note {
            s = s.line(format!("  {n}"));
        }
        if a.rows.is_empty() && a.error.is_none() {
            s = s.line("  пусто");
        }
        for r in &a.rows {
            let locked = if r.locked.is_zero() {
                String::new()
            } else {
                format!(", в ордерах {}", qty(r.locked))
            };
            let why = match &r.sell {
                Some(Err(w)) => format!(" — продать нельзя: {w}"),
                _ => String::new(),
            };
            s = s.line(format!("  {}: {}{locked}{why}", r.asset, qty(r.free)));
            if r.sell.is_some() {
                let i = refs.len();
                refs.push(Ref::Sell {
                    venue: b.venue.clone(),
                    account: a.account.clone(),
                    asset: r.asset.clone(),
                    free: r.free,
                });
                s = s.one(btn(
                    &format!("Продать {} ({})", r.asset, a.account),
                    &format!("s:{sid}.{i}"),
                ));
            }
        }
    }
    (
        s.one(btn("Обновить", &format!("k:bv.{}", b.venue)))
            .nav(Some("k:bal")),
        refs,
    )
}

/// Кошелёк DEX: BNB и USDT. Принимает: ответ кошелька, имя кошелька. Ошибок нет.
pub fn wallet(w: &WalletView, title: &str) -> Screen {
    let v = |x: Option<rust_decimal::Decimal>| x.map(qty).unwrap_or_else(|| "—".into());
    let mut s = Screen::new(format!(
        "Кошелёк DEX «{title}» — на {}",
        w.at.format("%H:%M:%S UTC")
    ))
    .line(format!("BNB: {}", v(w.bnb)))
    .line(format!(
        "{}: {}",
        w.quote
            .as_deref()
            .unwrap_or("USDT (venue.quote_asset DEX не задана)"),
        v(w.usdt)
    ));
    if let Some(e) = &w.error {
        s = s.line(format!("Не получено: {e}"));
    }
    s.one(btn("Обновить", &format!("k:bw.{}", w.wallet)))
        .nav(Some("k:bal"))
}
