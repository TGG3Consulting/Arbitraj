//! Денежный снимок проблемной записи. Единицы и знак берутся из учёта сделки, форматирование — только для вывода.
use crate::core::deal::Deal;
use crate::core::types::{StuckAlert, VenueId};
use crate::notify::human;
use crate::settings::{read::At, store::Store};
use rust_decimal::Decimal;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Incident {
    pub key: String,
    pub deal: String,
    pub order: Decimal,
    pub loss_usdt: Option<Decimal>,
    pub loss_pct: Option<Decimal>,
    pub text: String,
}

fn percent(amount: Decimal, invested: Decimal) -> Option<Decimal> {
    amount
        .checked_div(invested)?
        .checked_mul(Decimal::ONE_HUNDRED)
}

impl Incident {
    fn new(d: &Deal, key: String, title: &str, why: &str, loss: Option<Decimal>) -> Self {
        let text = human::Msg::new(human::STOP, title)
            .card(crate::core::deal_card::card(d))
            .reason(why)
            .id(&d.id)
            .render();
        Self {
            key,
            deal: d.id.clone(),
            order: d.invested,
            loss_usdt: loss,
            loss_pct: loss.and_then(|v| percent(v, d.invested)),
            text,
        }
    }

    pub fn alert(a: &StuckAlert, d: &Deal) -> Self {
        let mut i = Self::new(
            d,
            format!("a.{}", a.id),
            "Дежурному: застрявшая монета",
            a.reason.as_deref().unwrap_or("Ждёт решения"),
            a.loss_usdt,
        );
        i.loss_pct = a.loss_pct;
        i
    }

    pub fn halted(d: &Deal) -> Self {
        let loss = d.duty_halted_pnl_usdt.map(|pnl| -pnl);
        let prefix = crate::core::deal_book::end_prefix(crate::core::deal::Step::Halted);
        let stop = d.timeline.iter().rev().find(|(_, text)| {
            text.starts_with(&prefix) || text.starts_with(crate::core::ledger_load::INTERRUPTED)
        });
        let transition = stop.map_or(&d.started_at, |(at, _)| at);
        let why = stop
            .map(|(_, text)| text.clone())
            .unwrap_or_else(|| crate::core::manual_feed::last_note(d));
        Self::new(
            d,
            format!("d.{}@{transition}", d.id),
            "Дежурному: сделка остановлена",
            &why,
            loss,
        )
    }

    pub fn usdt(d: &Deal, at: &VenueId, why: &str, amount: Decimal) -> Self {
        let key = crate::core::usdt_return_parts::key(d, at);
        let reason = format!("{} на {}. {why}", human::usd(amount), human::venue(&at.0));
        let marker = crate::core::usdt_return_state::not_returned_text(&d.id, amount, at, why);
        let transition = d
            .timeline
            .iter()
            .rev()
            .find(|(_, note)| note == &marker)
            .map(|(time, _)| time)
            .unwrap_or(&d.started_at);
        Self::new(
            d,
            format!("u.{key}@{transition}"),
            "Дежурному: USDT не вернулись",
            &reason,
            crate::core::usdt_return_parts::get(d, Some(at)).and_then(|p| match p {
                crate::core::usdt_return_state::UsdtReturn::NotReturned {
                    result_at_failure,
                    ..
                } => result_at_failure.map(|pnl| -pnl),
                _ => None,
            }),
        )
    }

    pub fn message(&self) -> String {
        let mut text = format!("{}\nРазмер ордера: {}", self.text, human::usd(self.order));
        let loss = self.loss_usdt.or_else(|| {
            self.loss_pct?
                .checked_div(Decimal::ONE_HUNDRED)?
                .checked_mul(self.order)
        });
        if let Some(loss) = loss {
            text.push_str(&format!(
                "\nИтог на момент проблемы: {}",
                human::signed_usd(-loss)
            ));
            if let Some(pct) = self.loss_pct {
                text.push_str(&format!(" ({:+}%)", (-pct).round_dp(4)));
            }
        } else {
            text.push_str("\nИтог на момент проблемы: неизвестен");
        }
        text
    }
}

#[derive(Default)]
pub(super) struct Thresholds {
    pub pct: Option<Decimal>,
    pub usd: Option<Decimal>,
    pub order: Option<Decimal>,
}

impl Thresholds {
    pub fn read(settings: &Store) -> Self {
        let get = |k| {
            settings
                .decimal(k, At::default())
                .ok()
                .filter(|v| *v >= Decimal::ZERO)
        };
        Self {
            pct: get("telegram.duty_loss_pct"),
            usd: get("telegram.duty_loss_usdt"),
            order: get("telegram.duty_order_size_usdt"),
        }
    }

    pub fn configured(&self, settings: &Store) -> bool {
        (self.pct.is_some() || self.usd.is_some() || self.order.is_some())
            && settings
                .list("telegram.duty_chat_ids", At::default())
                .is_ok_and(|c| !c.is_empty())
    }

    pub fn qualifies(&self, i: &Incident) -> bool {
        if self.order.is_some_and(|minimum| i.order >= minimum) {
            return true;
        }
        let exceeds = |loss: Option<Decimal>, limit: Option<Decimal>| {
            loss.zip(limit)
                .is_some_and(|(v, t)| v > Decimal::ZERO && v >= t)
        };
        if exceeds(i.loss_pct, self.pct) {
            return true;
        }
        match i.loss_usdt {
            Some(usd) => exceeds(Some(usd), self.usd),
            // Ранее хранился только точный процент deal.loss_pct_if. Для границы 1 USDT из 3 сравниваем
            // два процента в исходной арифметике; умножение округлённой трети обратно даёт число ниже 1.
            None => exceeds(
                i.loss_pct,
                self.usd.and_then(|limit| percent(limit, i.order)),
            ),
        }
    }
}
