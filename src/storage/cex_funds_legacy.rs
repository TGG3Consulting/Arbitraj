//! Исходный запрос и доказанный предел обычной покупки на границе режимов.
use crate::{
    core::types::TradeRequest,
    gateway::{pending::PendingRef, settlement_intent::SpotFill},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
#[derive(Clone, Serialize, Deserialize)]
pub struct Prior {
    pub action: String,
    pub request: TradeRequest,
    pub owner: String,
    pub lookup: PendingRef,
    pub limit: Option<(String, Decimal)>,
    pub fill: Option<SpotFill>,
}
pub struct Legacy(Mutex<State>);
struct State {
    arm: bool,
    request: TradeRequest,
    owner: String,
    lookup: Option<PendingRef>,
    limit: Option<(String, Decimal)>,
    fill: Option<SpotFill>,
}
impl Legacy {
    pub(super) fn new(request: TradeRequest, owner: String) -> Self {
        Self(Mutex::new(State {
            arm: false,
            request,
            owner,
            lookup: None,
            limit: None,
            fill: None,
        }))
    }
    pub(crate) fn prepare(
        &self,
        p: &PendingRef,
        cap: Option<(String, Decimal)>,
    ) -> Result<(), String> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.arm {
            return Err("Режим счёта изменился до отправки обычного ордера".into());
        }
        s.lookup = Some(p.clone());
        s.limit = cap;
        Ok(())
    }
    pub(crate) fn result(&self, fill: Option<SpotFill>) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.fill = fill;
    }
    pub(super) fn prepared(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lookup
            .is_some()
    }
    pub(super) fn transitioned(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).arm
    }
    pub(super) fn snapshot(&self, key: &str, arm: bool) -> Option<Prior> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if arm {
            s.arm = true;
        }
        Some(Prior {
            action: key.to_owned(),
            request: s.request.clone(),
            owner: s.owner.clone(),
            lookup: s.lookup.clone()?,
            limit: s.limit.clone(),
            fill: s.fill.clone(),
        })
    }
}
