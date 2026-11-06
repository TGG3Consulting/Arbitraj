//! Файл: gateway/registry.rs
//! Что делает: реестр подключённых адаптеров — единая точка, через которую ядро
//! обращается к площадкам по их идентификатору.
//! Решение 200: реестр, созданный with_aliases, оборачивает каждую биржу в gateway/aliased.rs (склейка тикеров
//! одного адреса, coin.ticker_alias) — ядро и адаптеры об этом не знают.
//! Кто вызывает: main.rs (регистрирует адаптеры), ядро и API (получают адаптер).
//! От чего зависит: gateway/contract, gateway/errors, gateway/aliased.rs, core/types, settings, storage/journal.
//! Настройки: coin.ticker_alias (через aliased.rs).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::core::types::{DexAccountId, VenueId};

use super::contract::{Venue, VenueKind};
use super::errors::{ErrorCode, GatewayError, GatewayResult};

/// Реестр адаптеров. Дешёво клонируется (общий указатель).
#[derive(Clone, Default)]
pub struct Registry {
    venues: Arc<RwLock<HashMap<VenueId, Arc<dyn Venue>>>>,
    /// Решение 200: настройки и журнал для обёртки бирж (склейка тикеров); None — без обёртки (тесты).
    alias: Option<(
        crate::settings::store::Store,
        crate::storage::journal::Journal,
    )>,
}

impl Registry {
    /// Создаёт пустой реестр.
    pub fn new() -> Self {
        Self::default()
    }

    /// Реестр, который оборачивает биржи склейкой тикеров (решение 200, gateway/aliased.rs). Ошибок нет.
    pub fn with_aliases(
        settings: crate::settings::store::Store,
        journal: crate::storage::journal::Journal,
    ) -> Self {
        Self {
            venues: Default::default(),
            alias: Some((settings, journal)),
        }
    }

    /// Регистрирует адаптер (повторная регистрация заменяет старый); биржа — в обёртке склейки тикеров, если реестр
    /// создан with_aliases. Принимает: адаптер. Ошибок нет.
    pub async fn register(&self, venue: Arc<dyn Venue>) {
        let venue: Arc<dyn Venue> = match &self.alias {
            Some((s, j)) if venue.kind() == VenueKind::Cex => {
                Arc::new(super::aliased::Aliased::new(venue, s.clone(), j.clone()))
            }
            _ => venue,
        };
        self.venues.write().await.insert(venue.id(), venue);
    }

    /// Возвращает адаптер по идентификатору.
    /// Ошибка: VenueNotConnected — такой площадки нет в реестре.
    pub async fn get(&self, id: &VenueId) -> GatewayResult<Arc<dyn Venue>> {
        self.venues.read().await.get(id).cloned().ok_or_else(|| {
            GatewayError::new(
                ErrorCode::VenueNotConnected,
                Some(id.clone()),
                "Площадка не подключена",
            )
        })
    }

    /// Адаптер по идентификатору, привязанный к счёту DEX (решение 132): DEX — вид на кошелёк счёта
    /// (contract::for_account), биржа — сам адаптер (счёт ей не нужен). Принимает: id площадки, счёт (None — без
    /// привязки: DEX только читает). Ошибки: VenueNotConnected; NotConfigured — нет такого кошелька.
    pub async fn get_as(
        &self,
        id: &VenueId,
        account: Option<&DexAccountId>,
    ) -> GatewayResult<Arc<dyn Venue>> {
        let v = self.get(id).await?;
        match account {
            Some(a) if v.kind() == VenueKind::Dex => v.for_account(a),
            _ => Ok(v),
        }
    }

    /// Список идентификаторов подключённых площадок. Ошибок нет.
    pub async fn ids(&self) -> Vec<VenueId> {
        self.venues.read().await.keys().cloned().collect()
    }
}
