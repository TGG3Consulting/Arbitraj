//! Файл: settings/catalog/kraken.rs
//! Что делает: настройки, нужные только бирже Kraken (решение 172), — сколько записей читать в списках Funding (Beta),
//! сколько пар в одном запросе ставок TradeVolume. Значений нет.
//! Кто вызывает: settings/catalog/mod.rs.
//! От чего зависит: settings/catalog/mod.rs (короткая запись d и наборы областей).
//! Настройки: это и есть их список (зеркало docs/SETTINGS.md, раздел «Площадки — Kraken»). Решение 379 (Тигран
//! 17.11): venue.deposit_address_claim удалена — claim адреса депозита автоматический (venues/kraken/deposit_addr.rs).

use super::{V, d};
use crate::settings::schema::{Kind as K, Limit as L, SettingDef};

/// Наибольший limit списков Funding (Beta) — официально «limit (int32, >0, max 500)» у addresses, withdrawals, deposits
/// и deposit/addresses (OpenAPI https://docs.kraken.com/openapi/spot-rest.yaml).
const KRAKEN_LIST_MAX: u64 = 500;

/// Настройки решения 172. Порядок — как в docs/SETTINGS.md.
#[rustfmt::skip]
pub static KRAKEN: &[SettingDef] = &[
    d("Площадки", "venue.funding_list_limit", K::Integer, L::Positive, "записей", V, "Решение 172, только Kraken: сколько последних записей читать одной страницей в списках Funding (Beta) — выводы (GET /funding/v1/withdrawals), депозиты (/funding/v1/deposits), адреса вывода (/funding/v1/addresses), выданные адреса депозита (/funding/v2/deposit/addresses). Фильтра по id и хешу у Kraken нет — статус вывода и депозита ищется среди этих записей. Официально не больше 500. Не задана — вывод, статусы и адрес депозита Kraken не работают").max(KRAKEN_LIST_MAX),
    d("Площадки", "venue.fee_pairs_per_request", K::Integer, L::Positive, "пар", V, "Решение 172, только Kraken: сколько пар в одном запросе ставок комиссии POST /0/private/TradeVolume (pair — список через запятую; предел в документации не указан). Каждый запрос — +1 к счётчику REST ключа (Starter 15, убывание 0,33 в с) — чем больше пар в запросе, тем меньше запросов. Не задана — ставок нет, котировок Kraken нет"),
];
