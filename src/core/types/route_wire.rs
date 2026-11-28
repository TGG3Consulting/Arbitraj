//! Совместимость маршрутов журнала до явных типов Raindex и PancakeSwap X.
use super::{DexRoute, RouteMode};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Default, Serialize, Deserialize)]
enum RouteFormat {
    #[default]
    #[serde(skip)]
    Legacy,
    Typed,
}

impl RouteFormat {
    fn legacy(&self) -> bool { matches!(self, Self::Legacy) }
}

fn executable_default() -> bool { true }

/// Без промежуточного JSON: при записи доли и пояснение заимствуются, при чтении переходят в маршрут.
#[derive(Serialize, Deserialize)]
struct Fields<L, N> {
    mode: RouteMode,
    legs: L,
    block: u64,
    gas_units: u64,
    #[serde(default = "executable_default")]
    executable: bool,
    #[serde(default)]
    exec_note: N,
    #[serde(default, skip_serializing_if = "RouteFormat::legacy")]
    route_format: RouteFormat,
}

impl Serialize for DexRoute {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use RouteMode::*;
        // Общие режимы раньше разделяли исполнителей по пояснению. Пометка защищает
        // новый обычный билет при повторном чтении даже после изменения текста карточки.
        let route_format = match self.mode {
            Rfq(_) | RfqIndicative | Orderbook(_) | OrderbookIndicative | Relay(_) | RelayIndicative => RouteFormat::Typed,
            _ => RouteFormat::Legacy,
        };
        Fields {
            mode: self.mode, legs: &self.legs, block: self.block, gas_units: self.gas_units,
            executable: self.executable, exec_note: &self.exec_note, route_format,
        }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DexRoute {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let row = Fields::<Vec<super::RouteLeg>, Option<String>>::deserialize(deserializer)?;
        let mut route = DexRoute {
            mode: row.mode, legs: row.legs, block: row.block, gas_units: row.gas_units,
            executable: row.executable, exec_note: row.exec_note,
        };
        if row.route_format.legacy() && route.legs.is_empty() {
            use RouteMode::*;
            route.mode = match (route.mode, route.executable, route.exec_note.as_deref()) {
                (Rfq(id), true, Some("Raindex")) => Raindex(Some(id)),
                (RfqIndicative, false, Some("Raindex")) => Raindex(None),
                (Orderbook(id), true, Some("Raindex")) => RaindexSettlement(Some(id)),
                (OrderbookIndicative, false, Some("Raindex")) => RaindexSettlement(None),
                (Relay(id), true, Some("PancakeSwap X")) => PancakeX(Some(id)),
                (RelayIndicative, false, Some("PancakeSwap X")) => PancakeX(None),
                (mode, _, _) => mode,
            };
        }
        match route.mode {
            RouteMode::Raindex(ticket) | RouteMode::RaindexSettlement(ticket) | RouteMode::NativeRfqSettlement(ticket) | RouteMode::PancakeX(ticket)
                if !route.legs.is_empty() || route.executable != ticket.is_some() =>
            {
                Err(serde::de::Error::custom("Режим билета не соответствует долям или исполнимости маршрута"))
            }
            _ => Ok(route),
        }
    }
}

#[cfg(test)]
#[path = "route_wire_tests.rs"]
mod tests;
