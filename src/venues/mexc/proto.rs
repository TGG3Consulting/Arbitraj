//! Файл: venues/mexc/proto.rs
//! Что делает: сообщения Protocol Buffers WebSocket MEXC — только нужные поля, по официальным .proto
//! (https://github.com/mexcdevelop/websocket-proto, коммит 2026-09-09): обёртка PushDataV3ApiWrapper, частичный стакан
//! PublicLimitDepthsV3Api (тело 303), изменение баланса PrivateAccountV3Api (307), ордер PrivateOrdersV3Api (304).
//! Поля, которых здесь нет, prost пропускает (стандарт protobuf: неизвестные поля игнорируются).
//! Кто вызывает: depth.rs, account_update.rs.
//! От чего зависит: prost.
//! Настройки: нет.
//!
//! «The current websocket push uses the protobuf format» —
//! https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/protocol-buffers-integration

/// PushDataV3ApiWrapper.proto: channel = 1, oneof body, symbol = 3, symbolId = 4, createTime = 5, sendTime = 6.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Wrapper {
    #[prost(string, tag = "1")]
    pub channel: String,
    #[prost(oneof = "Body", tags = "303, 304, 307")]
    pub body: Option<Body>,
    #[prost(string, optional, tag = "3")]
    pub symbol: Option<String>,
    #[prost(int64, optional, tag = "5")]
    pub create_time: Option<i64>,
    #[prost(int64, optional, tag = "6")]
    pub send_time: Option<i64>,
}

/// Нужные варианты oneof body обёртки (номера — из PushDataV3ApiWrapper.proto).
#[derive(Clone, PartialEq, prost::Oneof)]
pub enum Body {
    /// publicLimitDepths = 303.
    #[prost(message, tag = "303")]
    LimitDepths(LimitDepths),
    /// privateOrders = 304.
    #[prost(message, tag = "304")]
    PrivateOrders(PrivateOrders),
    /// privateAccount = 307.
    #[prost(message, tag = "307")]
    PrivateAccount(PrivateAccount),
}

/// PublicLimitDepthsV3Api.proto: asks = 1, bids = 2, eventType = 3, version = 4, lastOrderCreateTime = 5.
#[derive(Clone, PartialEq, prost::Message)]
pub struct LimitDepths {
    #[prost(message, repeated, tag = "1")]
    pub asks: Vec<DepthItem>,
    #[prost(message, repeated, tag = "2")]
    pub bids: Vec<DepthItem>,
    #[prost(string, tag = "3")]
    pub event_type: String,
    #[prost(string, tag = "4")]
    pub version: String,
}

/// PublicLimitDepthV3ApiItem: price = 1, quantity = 2 (строки).
#[derive(Clone, PartialEq, prost::Message)]
pub struct DepthItem {
    #[prost(string, tag = "1")]
    pub price: String,
    #[prost(string, tag = "2")]
    pub quantity: String,
}

/// PrivateAccountV3Api.proto: vcoinName = 1, coinId = 2, balanceAmount = 3, balanceAmountChange = 4,
/// frozenAmount = 5, frozenAmountChange = 6, type = 7, time = 8.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PrivateAccount {
    #[prost(string, tag = "1")]
    pub vcoin_name: String,
    #[prost(string, tag = "3")]
    pub balance_amount: String,
    #[prost(string, tag = "4")]
    pub balance_amount_change: String,
    #[prost(string, tag = "5")]
    pub frozen_amount: String,
    #[prost(string, tag = "6")]
    pub frozen_amount_change: String,
    #[prost(string, tag = "7")]
    pub r#type: String,
    #[prost(int64, tag = "8")]
    pub time: i64,
}

/// PrivateOrdersV3Api.proto (нужные поля): id = 1, clientId = 2 — только для журнала (итог ордера — REST, trade.rs).
#[derive(Clone, PartialEq, prost::Message)]
pub struct PrivateOrders {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub client_id: String,
}
