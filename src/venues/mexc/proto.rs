//! Файл: venues/mexc/proto.rs
//! Что делает: сообщения Protocol Buffers WebSocket MEXC — только нужные поля, по официальным .proto
//! (https://github.com/mexcdevelop/websocket-proto, коммит 2026-09-09): обёртка PushDataV3ApiWrapper, частичный стакан
//! PublicLimitDepthsV3Api (тело 303), изменение баланса PrivateAccountV3Api (307), ордер PrivateOrdersV3Api (304),
//! изменения стакана PublicAggreDepthsV3Api (313) и сделки PublicAggreDealsV3Api (314) — решение 125, .proto того же
//! репозитория.
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
    #[prost(oneof = "Body", tags = "303, 304, 307, 313, 314")]
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
    /// publicAggreDepths = 313 (канал spot@public.aggre.depth.v3.api.pb, дополнение решения 125).
    #[prost(message, tag = "313")]
    AggreDepths(AggreDepths),
    /// publicAggreDeals = 314 (канал spot@public.aggre.deals.v3.api.pb, решение 125).
    #[prost(message, tag = "314")]
    AggreDeals(AggreDeals),
}

/// PublicAggreDepthsV3Api.proto: asks = 1, bids = 2, eventType = 3, fromVersion = 4, toVersion = 5
/// (уровни — PublicAggreDepthV3ApiItem: price = 1, quantity = 2, тот же вид, что DepthItem).
#[derive(Clone, PartialEq, prost::Message)]
pub struct AggreDepths {
    #[prost(message, repeated, tag = "1")]
    pub asks: Vec<DepthItem>,
    #[prost(message, repeated, tag = "2")]
    pub bids: Vec<DepthItem>,
    #[prost(string, tag = "3")]
    pub event_type: String,
    #[prost(string, tag = "4")]
    pub from_version: String,
    #[prost(string, tag = "5")]
    pub to_version: String,
}

/// PublicAggreDealsV3Api.proto: deals = 1, eventType = 2.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AggreDeals {
    #[prost(message, repeated, tag = "1")]
    pub deals: Vec<AggreDeal>,
    #[prost(string, tag = "2")]
    pub event_type: String,
}

/// PublicAggreDealsV3ApiItem: price = 1, quantity = 2, tradeType = 3 (1 — покупка, 2 — продажа), time = 4,
/// tradeId = 5.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AggreDeal {
    #[prost(string, tag = "1")]
    pub price: String,
    #[prost(string, tag = "2")]
    pub quantity: String,
    #[prost(int32, tag = "3")]
    pub trade_type: i32,
    #[prost(int64, tag = "4")]
    pub time: i64,
    #[prost(string, tag = "5")]
    pub trade_id: String,
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
