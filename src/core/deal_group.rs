//! Файл: core/deal_group.rs
//! Что делает: решение 394 (t179) — запись группы докупки в сделке (записка договорённостей 269, пп. 1, 10–13): к какой
//! группе относится сделка, ждёт ли её выручка общего возврата USDT, в какой сделке общий возврат, вышла ли сделка из
//! группы, итог группы у сделки с общим возвратом; ревью 3 t179: итог дошедших кусков общего возврата (Б1) и признак
//! «Итог группы ещё не отправлен» (Н4). Только данные и тексты; решения — coin_group.rs, usdt_return_pieces.rs,
//! group_notice.rs.
//! Группа — ID первой сделки (п. 1: «сделка 1 — главная»); вложенные сделки ссылаются на него.
//! Кто вызывает: deal.rs (поле Deal.group), executor.rs (создание), coin_group.rs и coin_group_flush.rs (возврат),
//! usdt_return_pieces.rs (куски), deal_return_cost.rs (затраты кусков), deal_text.rs и group_notice.rs (строки
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
    /// Ревью 3, Б1: общий возврат несколькими выводами по очереди (сумма выше максимума вывода) — итог дошедших кусков,
    /// пока возврат не закончен или если остаток не вернулся (их затраты — в итоге сделки, deal_return_cost::known).
    /// Дошли все — снимается, часть учёта «вернулись» на всю сумму с суммами по кускам.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pieces: Option<PiecesDone>,
    /// Ревью 3, Н4: у носителя общего возврата, уже известившего о своём закрытии, — «Итог группы» ещё не отправлен:
    /// уйдёт отдельным сообщением, когда возврат закончен (group_notice.rs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub total_due: bool,
}

/// Итог дошедших кусков общего возврата (ревью 3, Б1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PiecesDone {
    /// Сколько кусков дошло.
    pub n: u32,
    /// Запрошено к выводу по дошедшим кускам, USDT.
    pub amount: Decimal,
    /// Ушло с площадки (после округления вывода), USDT.
    pub sent: Decimal,
    /// Зачислено на площадке покупки, USDT.
    pub credited: Decimal,
    /// Затраты вывода не в USDT (газ кошелька DEX) в USDT; None — не было или не пересчитаны.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_usd: Option<Decimal>,
}

impl PiecesDone {
    /// Затраты дошедших кусков: ушло − зачислено + газ (как у части «вернулись», deal_return_cost::part_cost). Ошибок нет.
    pub fn cost(&self) -> Decimal {
        self.sent - self.credited + self.fee_usd.unwrap_or_default()
    }
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
            // Ревью 3, Н4: вышедшая из группы, которая вернула и выручку поздних сделок группы, — так и пишем.
            match &self.sum {
                Some(s) => {
                    out.push((
                        "Группа",
                        format!(
                            "была частью группы {} — свой возврат USDT, в нём и выручка сделок {}",
                            self.id,
                            s.deals.join(", ")
                        ),
                    ));
                    out.push((
                        "Итог этих сделок",
                        format!(
                            "{} сделок, {} USDT (один возврат USDT — в этой сделке)",
                            s.deals.len() + 1,
                            (s.others + result.unwrap_or_default()).round_dp(4)
                        ),
                    ));
                }
                None => out.push((
                    "Группа",
                    format!("была частью группы {} — свой возврат USDT", self.id),
                )),
            }
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
