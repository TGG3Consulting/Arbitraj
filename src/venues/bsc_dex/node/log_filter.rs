//! Файл: venues/bsc_dex/node/log_filter.rs
//! Что делает: проверка лога по фильтру logs в формате eth_subscribe / eth_getLogs ({address, topics}) — для событий
//! пулов из квитанций блоков (решение 68, node/receipts.rs): подписку WebSocket фильтрует узел, квитанции — фильтруются
//! здесь, теми же фильтрами, что строят node/subs.rs и pools/created.rs. Фильтр разбирается один раз при подписке
//! (множества адресов и topics), проверка лога — поиском в множествах.
//! Кто вызывает: node/receipts.rs.
//! От чего зависит: serde_json.
//! Настройки: нет.
//!
//! Правила фильтра — execution-apis filter.yaml: address — адрес или список (любой из); topics — по позициям, у каждой
//! null (любой), значение или список (любой из); в логе нет позиции, для которой есть условие, — не подходит
//! (https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/schemas/filter.yaml).
//! Пустой список (address: [] или позиция topics: []) — как у geth: условия нет, подходит любой (geth
//! eth/filters/filter.go filterLogs: «if len(addresses) > 0 …», пустая позиция topics пропускается). Спецификация про
//! пустой список молчит; наши фильтры (node/subs.rs, pools/created.rs) пустых списков не строят — пачка без пулов не
//! подписывается.

use std::collections::HashSet;

use serde_json::Value;

/// Разобранный фильтр: None — условия нет (любой).
#[derive(Debug, Clone, Default)]
pub struct Filter {
    address: Option<HashSet<String>>,
    topics: Vec<Option<HashSet<String>>>,
}

impl Filter {
    /// Разбирает фильтр logs. Принимает: {address?, topics?}. Ошибок нет: null и пустой список — условия нет.
    pub fn new(f: &Value) -> Filter {
        Filter {
            address: f
                .get("address")
                .filter(|v| !v.is_null())
                .map(set)
                .filter(|s| !s.is_empty()),
            topics: f
                .get("topics")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|c| Some(set(c)).filter(|s| !c.is_null() && !s.is_empty()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Подходит ли лог (поля address, topics). Ошибок нет.
    pub fn matches(&self, log: &Value) -> bool {
        let addr = log
            .get("address")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if self.address.as_ref().is_some_and(|s| !s.contains(&addr)) {
            return false;
        }
        let topics: Vec<String> = log
            .get("topics")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        self.topics.iter().enumerate().all(|(i, c)| match c {
            None => true,
            Some(s) => topics.get(i).is_some_and(|t| s.contains(t)),
        })
    }
}

/// Строка или список строк → множество в нижнем регистре. Ошибок нет.
fn set(v: &Value) -> HashSet<String> {
    match v {
        Value::String(s) => HashSet::from([s.to_ascii_lowercase()]),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_ascii_lowercase)
            .collect(),
        _ => HashSet::new(),
    }
}
