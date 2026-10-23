//! Файл: gateway/registry.rs
//! Что делает: реестр подключённых адаптеров — единая точка, через которую ядро
//! обращается к площадкам по их идентификатору.
//! Кто вызывает: main.rs (регистрирует адаптеры), ядро и API (получают адаптер).
//! От чего зависит: gateway/contract, gateway/errors, core/types.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::core::types::VenueId;

use super::contract::Venue;
use super::errors::{ErrorCode, GatewayError, GatewayResult};

/// Реестр адаптеров. Дешёво клонируется (общий указатель).
#[derive(Clone, Default)]
pub struct Registry {
    venues: Arc<RwLock<HashMap<VenueId, Arc<dyn Venue>>>>,
}

impl Registry {
    /// Создаёт пустой реестр.
    pub fn new() -> Self {
        Self::default()
    }

    /// Регистрирует адаптер (повторная регистрация заменяет старый).
    /// Принимает: адаптер. Ошибок нет.
    pub async fn register(&self, venue: Arc<dyn Venue>) {
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

    /// Список идентификаторов подключённых площадок. Ошибок нет.
    pub async fn ids(&self) -> Vec<VenueId> {
        self.venues.read().await.keys().cloned().collect()
    }
}
