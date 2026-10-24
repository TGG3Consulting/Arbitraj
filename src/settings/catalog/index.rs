//! Файл: settings/catalog/index.rs
//! Что делает: настройки справочника пулов от индексатора (решение 58): адрес API, сеть, ключ, темп, страницы,
//! частота обновления, подпись источника. Ликвидность индексатора ни в каких решениях не участвует (решение 58). Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Справочник пулов»).
//!
//! Адреса и заголовки ключа — https://docs.coingecko.com/docs/keyless-public-api ,
//! https://docs.coingecko.com/demo/reference/top-pools-contract-address ; подпись — https://brand.coingecko.com/resources/attribution-guide .

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Настройки справочника пулов. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static INDEX: &[SettingDef] = &[
    d("Справочник пулов", "index.api_url", K::Url, L::None, "URL", V, "Адрес API индексатора — откуда берётся список пулов монеты (решение 58; цены — только со своего узла). GeckoTerminal без ключа: https://api.geckoterminal.com/api/v2; CoinGecko Demo: https://api.coingecko.com/api/v3/onchain; Pro: https://pro-api.coingecko.com/api/v3/onchain — пути после адреса одинаковые, платный тариф = другой адрес + ключ"),
    d("Справочник пулов", "index.network", K::Text, L::None, "id сети", V, "id сети у индексатора (список — GET /networks): BSC — bsc"),
    d("Справочник пулов", "index.key_header", K::Choice, L::None, "заголовок", V, "Заголовок ключа CoinGecko по документации: Demo — x-cg-demo-api-key, Pro — x-cg-pro-api-key; сам ключ — секрет index.api_key. Не задан — запросы без ключа (GeckoTerminal)").choices(&["x-cg-demo-api-key", "x-cg-pro-api-key"]),
    d("Справочник пулов", "index.requests_per_min", K::Integer, L::Positive, "запросов в минуту", V, "Темп запросов к индексатору (каждая страница — запрос; ошибки тоже считаются). По документации CoinGecko: без ключа GeckoTerminal ≈10, Demo — 100 (и 10 000 в месяц), Basic — 300. Ответ 429 — правило шлюза «Превышен лимит запросов» площадки bsc_dex (то же, что у узла BSC: отдельной области правил для индексатора нет), нет его — общее"),
    d("Справочник пулов", "index.page_size", K::Integer, L::Positive, "пулов", V, "Сколько пулов на странице отдаёт индексатор (документация: «up to 20 pools per page»): страница короче — последняя, следующая не запрашивается"),
    d("Справочник пулов", "index.pages_per_token", K::Integer, L::Positive, "страниц", V, "Сколько страниц пулов спрашивать по монете (по ликвидности и объёму, самые крупные — первыми). Документация: больше 10 страниц — только с тарифа Analyst"),
    d("Справочник пулов", "index.refresh_s", K::Integer, L::Positive, "секунды", V, "Как часто спрашивать индексатор по каждой монете и список DEX (после перезапуска — от времени последнего ответа в SQLite). Новые пулы между обновлениями ловятся событиями создания пула своих фабрик через узел. Бюджет Demo: монет × страниц × (30 дней / интервал) ≤ 10 000"),
    d("Справочник пулов", "index.attribution_text", K::Text, L::None, "текст", V, "Подпись источника данных на экранах, если её требуют условия индексатора (гайд CoinGecko: «On-chain data provided by GeckoTerminal»). Не задана — не показывается"),
    d("Справочник пулов", "index.attribution_url", K::Url, L::None, "URL", V, "Ссылка подписи источника (гайд CoinGecko: https://www.geckoterminal.com)"),
];
