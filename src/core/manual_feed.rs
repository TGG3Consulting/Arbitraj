//! Файл: core/manual_feed.rs
//! Что делает: записи экрана «Ручное управление» с их кнопками (решение 156): алерты застрявших монет, монеты
//! «Беру на себя», остановленные сделки, «USDT не вернулись» (решение 163, manual_usdt.rs). Кнопки, команды ядра, вопросы подтверждения и недоступность — ровно как
//! на экране (ui/src/pages/Manual.svelte рисует кнопки из этих же записей), поэтому Telegram (notify/mirror*.rs)
//! получает те же кнопки и те же команды, второй логики нет.
//! Кто вызывает: manual_return.rs (Core::manual → GET /api/manual), notify (Core::manual_records).
//! От чего зависит: core/types/manual_feed.rs, manual_rows.rs (алерты и «Беру на себя»), manual_usdt.rs, deal.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{ManualAction as A, ManualHolding, ManualRecord, RecordKind, StuckAlert};

use crate::notify::human::{Msg, STOP, usd};

use super::deal::Deal;
use super::deal_text;
use super::engine::{Core, lk};
use super::manual_rows::{alert_record, btn, holding_record, sold};
use super::usdt_return_state::UsdtReturn;

/// Последняя запись хронологии сделки без «Предварительный убыток» и «В итог дня» — причина остановки.
fn last_note(d: &Deal) -> String {
    d.timeline
        .iter()
        .rev()
        .map(|(_, x)| x)
        .find(|x| !x.starts_with("Предварительный убыток") && !x.starts_with("В итог дня"))
        .cloned()
        .unwrap_or_default()
}

/// Запись остановленной сделки: «Продать сейчас», «Продать и перегнать», «Продано вручную»; недоступность —
/// как на экране: монет нет — обе продажи недоступны. Решение 234: монеты на площадке покупки — «Продать и перегнать»
/// доступна (лучшая площадка может быть другой; продано здесь — перегон USDT не нужен).
pub fn halted_record(d: &Deal) -> ManualRecord {
    let (t, v, buy) = (&d.direction.asset.ticker, &d.at.0, &d.direction.buy_on.0);
    let empty = (d.qty <= Decimal::ZERO)
        .then(|| "монет в сделке нет — закройте её «Продано вручную»".to_string());
    let est = d
        .provisional_usd
        .map(crate::notify::human::signed_usd)
        .unwrap_or_else(|| "—".into());
    let full = last_note(d);
    ManualRecord {
        key: ManualRecord::key_of(RecordKind::Halted, &d.id),
        kind: RecordKind::Halted,
        object_id: d.id.clone(),
        deal_id: d.id.clone(),
        venue: v.clone(),
        text: Msg::new(STOP, "Сделка остановлена — ждёт разбора")
            .line(deal_text::coin_route(d))
            .kv("Монеты", deal_text::coins_at(d))
            .kv("Вложено", usd(d.invested))
            .kv("В итоге дня", &est)
            .reason(&full)
            .id(&d.id)
            .render(),
        reason: crate::notify::reason::short(&full),
        reason_full: full.clone(),
        buttons: vec![
            btn(
                "sell_now",
                "Продать сейчас",
                A::SellHalted {
                    deal_id: d.id.clone(),
                },
                format!(
                    "Продать {t} сделки {} на {v} по рынку сейчас? Количество — по реальному балансу, порог убытка не проверяется.",
                    d.id
                ),
                empty.clone(),
                false,
            ),
            btn(
                "sell_ret",
                "Продать и перегнать",
                A::SellHaltedReturn {
                    deal_id: d.id.clone(),
                },
                format!(
                    "Найти площадку, где {t} сделки {} даст больше всего USDT после всех затрат перевода (или продать здесь, на {v}), перегнать туда, продать по рынку и вернуть USDT на {buy}? Порог убытка не проверяется.",
                    d.id
                ),
                empty,
                false,
            ),
            sold(&d.id, t, d.qty),
        ],
    }
}

impl Core {
    /// Все записи экрана «Ручное управление» с кнопками: алерты, «Беру на себя», остановленные сделки, «USDT не
    /// вернулись» — в порядке
    /// таблиц экрана; внутри вида — по времени (старые сверху), остановленные — как Core::halted. Ошибок нет.
    pub fn manual_records(&self) -> Vec<ManualRecord> {
        let mut alerts: Vec<StuckAlert> = lk(&self.inner.alerts)
            .values()
            .map(|(a, _)| a.clone())
            .collect();
        alerts.sort_by_key(|a| a.at);
        let mut holdings: Vec<ManualHolding> = lk(&self.inner.holdings)
            .values()
            .map(|(h, _)| h.clone())
            .collect();
        holdings.sort_by_key(|h| h.taken_at);
        let mut v: Vec<ManualRecord> = alerts.iter().map(alert_record).collect();
        v.extend(holdings.iter().map(holding_record));
        v.extend(self.halted().iter().map(halted_record));
        // Решение 163: «USDT не вернулись» — после остановленных (порядок таблиц экрана).
        // Решение 227: запись на каждую часть возврата «не вернулись».
        v.extend(
            self.usdt_not_returned()
                .iter()
                .flat_map(super::manual_usdt::usdt_records),
        );
        v
    }

    /// Закрытые сделки, чьи USDT не вернулись (таблица «USDT не вернулись — ждут перегона»), старые сверху. Решение 227:
    /// сделка попадает, если хотя бы одна часть возврата «не вернулись». Ошибок нет.
    pub fn usdt_not_returned(&self) -> Vec<Deal> {
        let mut v: Vec<Deal> = lk(&self.inner.deals)
            .values()
            .filter(|d| {
                super::usdt_return_parts::all(d)
                    .iter()
                    .any(|p| matches!(p, UsdtReturn::NotReturned { .. }))
            })
            .cloned()
            .collect();
        v.sort_by_key(|d| d.updated_at);
        v
    }
}
