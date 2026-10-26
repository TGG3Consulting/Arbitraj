//! Файл: core/card_links.rs
//! Что делает: ссылки карточки сигнала (решение 74) — на пару на бирже и на пул DEX — по шаблонам из настроек.
//! Готовых адресов в коде нет. Шаблон пары — venue.pair_url_template площадки-биржи: {base} — тикер монеты, {quote} —
//! котируемая монета этой площадки (venue.quote_asset), {symbol} — они же подряд (BTCUSDT; символа API ядро не знает);
//! шаблон пула — dex.pool_url_template, область «DEX» (своя строка у каждой DEX индексатора — страницы пулов у DEX разные:
//! GeckoTerminal открывает пулы PancakeSwap V2 и Infinity CL, но не Uniswap V4 — проверено в браузере 2026-10-26):
//! {pool} — адрес пула (V2/V3) или PoolId (V4). Как биржа пишет пару
//! в адресе, решает сам шаблон: у BingX страница https://bingx.com/en/spot/BTCUSDT (проверено в браузере 2026-10-26;
//! BTC_USDT ведёт туда же) — шаблон «https://bingx.com/en/spot/{symbol}». Не задан шаблон — ссылки нет, рядом
//! «шаблон ссылки не задан».
//! Шаблон — только https; подстановка, которой нет в списке, — ошибка с её именем (не догадка).
//! Кто вызывает: core/card.rs, тесты (tests/core_signal_card.rs).
//! От чего зависит: settings, core/types (PoolRef).
//! Настройки: venue.pair_url_template, dex.pool_url_template, venue.quote_asset.

use serde::Serialize;

use crate::core::types::PoolRef;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::settings::value::Scope;

/// Ссылка карточки: адрес или почему его нет.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Link {
    pub venue: String,
    /// «pair» — пара на бирже, «pool» — пул DEX.
    pub kind: &'static str,
    /// Что за ссылка (тикер пары или адрес пула) — подпись в интерфейсе.
    pub label: String,
    pub url: Option<String>,
    /// Почему ссылки нет («шаблон ссылки не задан (venue.pair_url_template)», ошибка шаблона).
    pub why: Option<String>,
}

/// Что делает: подставляет значения в шаблон.
/// Принимает: шаблон, пары (имя подстановки без скобок, значение). Возвращает: адрес.
/// Ошибки: текст — не https, неизвестная или незакрытая подстановка.
pub fn fill(template: &str, vars: &[(&str, &str)]) -> Result<String, String> {
    if !template.starts_with("https://") {
        return Err(format!("шаблон «{template}» должен начинаться с https://"));
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let Some(j) = tail.find('}') else {
            return Err(format!("в шаблоне «{template}» незакрытая «{{»"));
        };
        let name = &tail[..j];
        let Some((_, v)) = vars.iter().find(|(k, _)| *k == name) else {
            let known: Vec<String> = vars.iter().map(|(k, _)| format!("{{{k}}}")).collect();
            return Err(format!(
                "в шаблоне «{template}» неизвестная подстановка {{{name}}} (есть: {})",
                known.join(", ")
            ));
        };
        out.push_str(&url_part(v));
        rest = &tail[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Значение для адреса: буквы, цифры и «-_.~» как есть, остальное — %XX (RFC 3986, unreserved). Ошибок нет.
fn url_part(v: &str) -> String {
    let mut s = String::new();
    for b in v.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Что делает: ссылка на пару биржи. Принимает: настройки, площадку, тикер. Возвращает: Link (ошибок нет — в why).
pub fn pair(s: &Store, venue: &str, ticker: &str) -> Link {
    let at = At {
        venue: Some(venue),
        coin: None,
    };
    let quote = s.text("venue.quote_asset", at);
    let r = template(s, "venue.pair_url_template", venue).and_then(|t| {
        let q = quote.as_ref().map_err(|e| e.message.clone())?;
        let symbol = format!("{ticker}{q}");
        fill(&t, &[("base", ticker), ("quote", q), ("symbol", &symbol)])
    });
    let label = match &quote {
        Ok(q) => format!("{ticker}/{q}"),
        Err(_) => ticker.to_string(),
    };
    link(venue, "pair", label, r)
}

/// Что делает: ссылка на пул DEX. Принимает: настройки, площадку, id области «DEX» пула (вид пула у адаптера), пул.
/// Возвращает: Link.
pub fn pool(s: &Store, venue: &str, dex: &str, p: &PoolRef) -> Link {
    let id = pool_id(p);
    let name = "dex.pool_url_template";
    let r = s
        .text_in(name, &Scope::Dex(dex.to_string()))
        .map_err(|e| unset(e, &format!("{name}, DEX {dex}")))
        .and_then(|t| fill(&t, &[("pool", &id)]));
    link(venue, "pool", id, r)
}

/// Адрес пула (V2/V3) или PoolId (V4) строкой 0x…. Ошибок нет.
pub fn pool_id(p: &PoolRef) -> String {
    match p {
        PoolRef::Contract(a) => format!("{a:#x}"),
        PoolRef::Singleton { id, .. } => format!("{id:#x}"),
    }
}

/// Шаблон площадки. Ошибки: текст — «шаблон ссылки не задан (имя)» или ошибка чтения.
fn template(s: &Store, name: &str, venue: &str) -> Result<String, String> {
    let at = At {
        venue: Some(venue),
        coin: None,
    };
    s.text(name, at).map_err(|e| unset(e, name))
}

/// Ошибка чтения шаблона текстом: «шаблон ссылки не задан (где)» или как есть. Ошибок нет.
fn unset(e: crate::gateway::errors::GatewayError, place: &str) -> String {
    if e.code == ErrorCode::NotConfigured {
        format!("шаблон ссылки не задан ({place})")
    } else {
        e.message
    }
}

/// Собирает Link из итога. Ошибок нет.
fn link(venue: &str, kind: &'static str, label: String, r: Result<String, String>) -> Link {
    let (url, why) = match r {
        Ok(u) => (Some(u), None),
        Err(e) => (None, Some(e)),
    };
    Link {
        venue: venue.to_string(),
        kind,
        label,
        url,
        why,
    }
}
