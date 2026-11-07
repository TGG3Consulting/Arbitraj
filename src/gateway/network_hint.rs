//! Файл: gateway/network_hint.rs
//! Что делает: решение 200 — имена сетей монеты у биржи из её живого справочника для кнопки «Проверить подключение»,
//! когда venue.bsc_network не задана (пример 06.11: bybit «Справочник сетей не обновлён: Не задана настройка
//! venue.bsc_network»). Общий механизм для всех бирж: запрос сетей одной монеты (coin_networks_of контракта) идёт в
//! режиме подсказки settings/hint.rs — незаданная venue.bsc_network читается пустой, адаптер отдаёт сети без BSC, и
//! из них берутся имена (поле network ядра — то, с чем ядро сравнивает venue.bsc_network).
//! Тот же текст подсказки дописывается к ошибке справочника сетей «не задана venue.bsc_network» (журнал core.registry,
//! core/registry_sync.rs): Тигран сразу видит, какое имя задать.
//! Монета подсказки — котируемая монета биржи (venue.quote_asset, USDT): у неё сеть BSC есть на каждой бирже.
//! Кто вызывает: api/check.rs (venue), core/registry_sync.rs (ошибка справочника сетей).
//! От чего зависит: gateway/contract.rs, gateway/errors.rs, settings/hint.rs, settings/read.rs.
//! Настройки: venue.bsc_network (только если не задана), venue.quote_asset (по площадке).

use crate::settings::read::At;
use crate::settings::store::Store;

use super::contract::Venue;
use super::errors::{ErrorCode, GatewayError, GatewayResult};

/// Ошибка — «не задана venue.bsc_network». Ошибок нет.
pub fn bsc_unset(e: &GatewayError) -> bool {
    e.code == ErrorCode::NotConfigured
        && (e.setting.as_deref() == Some(crate::settings::hint::BSC_NETWORK)
            || e.message.contains(crate::settings::hint::BSC_NETWORK))
}

/// Текст подсказки для биржи без venue.bsc_network: имена сетей котируемой монеты из живого справочника или причина,
/// почему их нет. Принимает: адаптер, настройки. Ошибок нет: сбой — текстом.
pub async fn text(v: &dyn Venue, settings: &Store) -> String {
    let id = v.id().0;
    let at = At {
        venue: Some(&id),
        coin: None,
    };
    let quote = match settings.text("venue.quote_asset", at) {
        Ok(q) => q,
        Err(e) => {
            return format!(
                "venue.bsc_network не задана; имена сетей показать нельзя: {}",
                e.message
            );
        }
    };
    match names(v, &quote).await {
        Ok(n) if !n.is_empty() => format!(
            "venue.bsc_network не задана — сети {quote} у {id}: {}; задайте имя сети BSC из этого списка",
            n.join(", ")
        ),
        Ok(_) => format!("venue.bsc_network не задана; у {id} нет сетей {quote} в справочнике"),
        Err(e) => format!(
            "venue.bsc_network не задана; справочник сетей {quote} не получен: {}",
            e.message
        ),
    }
}

/// Имена сетей монеты у биржи по её живому справочнику, без повторов, по алфавиту. Принимает: адаптер, тикер.
/// Ошибки: как у coin_networks_of адаптера (связь, ключ, другая незаданная настройка).
pub async fn names(v: &dyn Venue, ticker: &str) -> GatewayResult<Vec<String>> {
    let list = crate::settings::hint::NETWORK_NAMES_HINT
        .scope((), v.coin_networks_of(ticker))
        .await?;
    let mut out: Vec<String> = list.into_iter().map(|i| i.network).collect();
    out.sort();
    out.dedup();
    Ok(out)
}
