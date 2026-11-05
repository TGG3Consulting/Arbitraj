//! Файл: core/usdt_sources.rs
//! Что делает: решение 176 — списки «Откуда» и «Куда» ручного перевода USDT: биржи реестра и кошельки DEX с экрана
//! «Маппинг кошельков» (решение 132), у каждой — свободные USDT торгового счёта (у кошелька — USDT кошелька) из памяти
//! панели «Балансы» (balances_cache.rs: запрос к площадке — только если ответ старше допустимого возраста).
//! «Откуда» — только с USDT больше нуля; «Куда» — все, кроме «откуда»; от кошелька — только биржи (кошелёк ↔
//! кошелёк ядро не переводит, usdt_transfer_plan::account).
//! Кто вызывает: api/usdt_transfer.rs (интерфейс), notify/ctl/transfer*.rs (Telegram).
//! От чего зависит: balances_cache.rs, usdt_transfer_plan.rs (End), settings (accounts, venue.quote_asset),
//! notify/human.rs (имя биржи).
//! Настройки: venue.quote_asset (тикер USDT по площадке), ui.balances_refresh_ms (возраст — передаёт вызывающий).

use std::time::Duration;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::DexAccountId;
use crate::settings::read::At;

use super::engine::Core;
use super::usdt_transfer_check::End;

/// Площадка перевода с остатком USDT.
#[derive(Debug, Clone)]
pub struct Source {
    pub end: End,
    /// Как показать человеку: «BingX», «Кошелёк DEX «MEXC» (mexc)».
    pub label: String,
    /// Свободно USDT; None — не прочитано (причина в error).
    pub usdt: Option<Decimal>,
    pub error: Option<String>,
}

impl Source {
    /// JSON для интерфейса. Ошибок нет.
    pub fn to_json(&self) -> Value {
        json!({ "venue": self.end.venue.0, "wallet": self.end.wallet.as_ref().map(|w| w.0.clone()),
            "key": self.end.key(), "label": self.label, "usdt": self.usdt, "error": self.error })
    }
}

/// Все площадки перевода с балансом USDT. Принимает: ядро, допустимый возраст ответа (None — запрос сейчас).
/// Возвращает: биржи по id, затем кошельки по имени. Ошибок нет: сбой — в Source::error.
pub async fn all(core: &Core, max_age: Option<Duration>) -> Vec<Source> {
    let mut out = Vec::new();
    for v in core.cex_ids() {
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        let quote = core.inner.deps.settings.text("venue.quote_asset", at);
        let (usdt, error) = match (core.venue_balances(&v, max_age).await, quote) {
            (Err(e), _) => (None, Some(e)),
            (_, Err(e)) => (None, Some(e.message)),
            (Ok(b), Ok(q)) => match (b.trading_free(&q), b.error) {
                (Some(x), _) => (Some(x), None),
                (None, e) => (
                    None,
                    Some(e.unwrap_or_else(|| "торговый счёт не прочитан".into())),
                ),
            },
        };
        let label = crate::notify::human::venue(&v.0);
        out.push(Source {
            end: End {
                venue: v,
                wallet: None,
            },
            label,
            usdt,
            error,
        });
    }
    let Some(dex) = core.dex_id() else {
        return out;
    };
    let accounts = core.inner.deps.settings.accounts();
    let mut wallets = accounts.wallets();
    wallets.sort_by(|a, b| a.name.cmp(&b.name));
    for w in wallets {
        let rows = accounts.rows_of(&w.id).join(", ");
        let label = if rows.is_empty() {
            format!("Кошелёк DEX «{}»", w.name)
        } else {
            format!("Кошелёк DEX «{}» ({rows})", w.name)
        };
        let (usdt, error) = match core.wallet_balances(&w.id, max_age).await {
            Ok(b) => (b.usdt, b.error),
            Err(e) => (None, Some(e)),
        };
        let end = End {
            venue: dex.clone(),
            wallet: Some(DexAccountId(w.id)),
        };
        out.push(Source {
            end,
            label,
            usdt,
            error,
        });
    }
    out
}

/// «Откуда»: площадки с USDT больше нуля. Принимает: все площадки. Ошибок нет.
pub fn from_list(all: &[Source]) -> Vec<&Source> {
    all.iter()
        .filter(|s| s.usdt.is_some_and(|u| u > Decimal::ZERO))
        .collect()
}

/// «Куда»: все, кроме «откуда»; от кошелька — только биржи. Принимает: все площадки, «откуда». Ошибок нет.
pub fn to_list<'a>(all: &'a [Source], from: &End) -> Vec<&'a Source> {
    all.iter()
        .filter(|s| s.end != *from && (from.wallet.is_none() || s.end.wallet.is_none()))
        .collect()
}

/// Имя площадки перевода для человека. Принимает: все площадки, конец. Ошибок нет: нет в списке — ключ.
pub fn label_of(all: &[Source], end: &End) -> String {
    all.iter()
        .find(|s| s.end == *end)
        .map(|s| s.label.clone())
        .unwrap_or_else(|| end.key())
}

/// Площадка по ключу End::key (из короткого номера кнопки Telegram или из интерфейса). Ошибок нет.
pub fn by_key<'a>(all: &'a [Source], key: &str) -> Option<&'a Source> {
    all.iter().find(|s| s.end.key() == key)
}
