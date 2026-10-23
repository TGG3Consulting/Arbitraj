//! Файл: core/intermediate.rs
//! Что делает: промежуточные монеты (решение 52) — переключатель и список адресов BSC по площадке DEX:
//! включено — монета допускается и с пулом только с промежуточной монетой (маршрут «USDT → промежуточная →
//! монета» в одной транзакции строит адаптер DEX); выключено — только пулы с USDT. Включено, а список
//! не задан или пуст — честное «не задано»: такая монета не допускается (не торгуем по USDT молча вместо).
//! Тикер промежуточной монеты — по адресу контракта из справочника бирж (для живой цены и подписи).
//! Кто вызывает: core/rebuild.rs (справочник), core/subscribe.rs (стаканы бирж), core/coins_view.rs.
//! От чего зависит: settings, core/types.
//! Настройки: dex.intermediate_enabled, dex.intermediate_tokens (по площадке DEX).

use std::collections::HashMap;

use alloy_primitives::Address;
use serde::Serialize;

use crate::core::types::{CoinNetworkInfo, VenueId};
use crate::settings::read::At;
use crate::settings::store::Store;

/// Состояние настройки на площадке DEX.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Intermediate {
    /// Выключено (или не задано переключение — тогда тоже только USDT; причина в NotConfigured нет,
    /// т.к. выключено — значит маршрут только через USDT по решению 52).
    Off,
    /// Включено, список адресов.
    On(Vec<Address>),
    /// Не задано: переключатель или (при включённом) список — текст причины.
    NotConfigured(String),
}

/// Читает настройку площадки DEX. Принимает: хранилище, площадку. Возвращает: Intermediate.
/// Переключатель не задан — NotConfigured (значений по умолчанию нет).
pub fn read(s: &Store, venue: &VenueId) -> Intermediate {
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    match s.flag("dex.intermediate_enabled", at) {
        Err(e) => Intermediate::NotConfigured(e.message),
        Ok(false) => Intermediate::Off,
        Ok(true) => match s.addresses("dex.intermediate_tokens", at) {
            Ok(list) if !list.is_empty() => Intermediate::On(list),
            Ok(_) => Intermediate::NotConfigured(
                "dex.intermediate_enabled = да, а список dex.intermediate_tokens пуст — задайте адреса".into(),
            ),
            Err(e) => Intermediate::NotConfigured(e.message),
        },
    }
}

/// Тикер токена по адресу контракта — из справочника бирж. None — ни одна биржа не знает этот адрес.
pub fn ticker_of(
    networks: &HashMap<VenueId, Vec<CoinNetworkInfo>>,
    addr: Address,
) -> Option<String> {
    networks
        .values()
        .flatten()
        .find(|i| i.asset.bsc_contract == Some(addr))
        .map(|i| i.asset.ticker.clone())
}

/// Подпись промежуточной монеты: тикер (если биржа знает адрес) и адрес.
pub fn label(networks: &HashMap<VenueId, Vec<CoinNetworkInfo>>, addr: Address) -> String {
    match ticker_of(networks, addr) {
        Some(t) => format!("{t} ({addr})"),
        None => addr.to_string(),
    }
}
