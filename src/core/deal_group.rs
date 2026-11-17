//! Файл: core/deal_group.rs
//! Что делает: решение 390 (t179) — запись группы докупки в сделке (записка договорённостей 269, пп. 1, 10–13): к какой
//! группе относится сделка, ждёт ли её выручка общего возврата USDT, в какой сделке общий возврат, вышла ли сделка из
//! группы, итог группы у сделки с общим возвратом. Только данные и тексты; решения — coin_group.rs.
//! Группа — ID первой сделки (п. 1: «сделка 1 — главная»); вложенные сделки ссылаются на него.
//! Кто вызывает: deal.rs (поле Deal.group), executor.rs (создание), coin_group.rs (возврат), deal_text.rs (строки
//! Telegram), ledger_load.rs (перезапуск), интерфейс (поле group в /api/deals).
//! От чего зависит: core/types.
//! Настройки: нет (включатель exec.top_up — coin_ride.rs).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::VenueId;

/// Группа докупки сделки.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DealGroup {
    /// ID первой сделки группы (у первой — её собственный).
    pub id: String,
    /// Выручка этой сделки ждёт общего возврата USDT группы (п. 10): откуда и сколько. Ставит coin_group.rs при
    /// закрытии, пока идут другие сделки группы; снимается, когда общий возврат начат (shared_in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<(VenueId, Decimal)>,
    /// В какой сделке общий возврат USDT группы (п. 11): у вложенных — её ID, в строке возврата — 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_in: Option<String>,
    /// Сделка вышла из группы (п. 13: зависла или закрылась не продажей на площадке плана) — свой возврат USDT.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub left: bool,
    /// У сделки с общим возвратом: сделки группы, чья выручка в нём, и сумма их итогов (без этой сделки) — итог
    /// группы = эта сумма + итог этой сделки (в нём один общий возврат, п. 12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sum: Option<GroupSum>,
}

/// Итог группы у сделки с общим возвратом.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupSum {
    /// ID других сделок группы, чья выручка вошла в общий возврат.
    pub deals: Vec<String>,
    /// Сумма их итогов, USDT (у каждой возврат 0).
    pub others: Decimal,
    /// Сколько USDT их выручки вошло в общий возврат (потолок повтора и «Перегнать» — usdt_return_deal::own_cap).
    pub usdt: Decimal,
}

impl DealGroup {
    /// Новая запись группы. Принимает: ID первой сделки группы. Ошибок нет.
    pub fn new(id: &str) -> Self {
        DealGroup {
            id: id.to_string(),
            ..Default::default()
        }
    }

    /// Строки уведомления deal.closed (п. 12): группа, общий возврат, итог группы. Принимает: ID сделки, её итог.
    /// Возвращает: пары «заголовок — значение». Ошибок нет.
    pub fn lines(&self, deal_id: &str, result: Option<Decimal>) -> Vec<(&'static str, String)> {
        let mut out = vec![];
        if self.left {
            out.push((
                "Группа",
                format!("была частью группы {} — свой возврат USDT", self.id),
            ));
            return out;
        }
        if self.id != deal_id || self.sum.is_some() || self.wait.is_some() {
            out.push(("Группа", format!("докупка, группа {}", self.id)));
        }
        if let Some(x) = &self.shared_in {
            out.push(("Возврат USDT", format!("0 — общий возврат в сделке {x}")));
        } else if self.wait.is_some() {
            out.push((
                "Возврат USDT",
                "0 — общий возврат группы после последней её сделки".to_string(),
            ));
        }
        if let Some(s) = &self.sum {
            let total = s.others + result.unwrap_or_default();
            out.push((
                "Итог группы",
                format!(
                    "{} сделок, {} USDT (один общий возврат USDT — в этой сделке)",
                    s.deals.len() + 1,
                    total.round_dp(4)
                ),
            ));
        }
        out
    }
}
