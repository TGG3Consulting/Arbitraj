//! Запрос цены из Telegram. В памяти до подтверждения хранится конкретный выбор
//! по записи. Новый расчёт этой записи заменяет предыдущий. После перезапуска
//! старые кнопки не могут создать новую продажу.
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, ManualAction, RecordKind, VenueId};
use crate::settings::read::At;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::{manual_price, manual_price_exec};

const STALE: &str = "Расчёт устарел. Нажмите «Запрос цены» ещё раз.";

pub struct SalePriceReply {
    pub text: String,
    pub best_venue: Option<String>,
    pub at_origin: bool,
    pub here: Option<ManualAction>,
    pub away: Option<ManualAction>,
}

#[derive(Clone)]
pub(super) struct Ticket {
    id: String,
    started: Instant,
    deal: Deal,
    qty: Decimal,
    here: bool,
    here_wallet: Option<DexAccountId>,
    target: Option<(VenueId, Option<DexAccountId>)>,
}

fn record(core: &Core, key: &str) -> Result<(RecordKind, Deal), String> {
    let (prefix, id) = key.split_once('.').ok_or(STALE)?;
    match prefix {
        "a" => lk(&core.inner.alerts)
            .get(id)
            .map(|(_, d)| (RecordKind::Alert, d.clone())),
        "d" => lk(&core.inner.deals)
            .get(id)
            .filter(|d| d.step == Step::Halted)
            .map(|d| (RecordKind::Halted, d.clone())),
        _ => None,
    }
    .ok_or_else(|| STALE.into())
}

async fn estimate(core: &Core, deal: &Deal) -> Result<(Decimal, manual_price::Comparison), String> {
    let qty = manual_price_exec::quote_qty(core, deal).await?;
    let mut position = deal.clone();
    position.qty = qty;
    Ok((qty, manual_price::compare(core, &position).await?))
}

impl Core {
    fn quote_ttl(&self) -> Result<Duration, String> {
        self.inner
            .deps
            .settings
            .integer("notify.dialog_ttl_s", At::default())
            .map(Duration::from_secs)
            .map_err(|e| e.message)
    }

    pub async fn sale_price_query(&self, key: &str) -> Result<SalePriceReply, String> {
        let started = Instant::now();
        let (_, deal) = record(self, key)?;
        let (qty, rates) = estimate(self, &deal).await?;
        if !manual_price_exec::matches(&deal, &record(self, key)?.1) {
            return Err(STALE.into());
        }
        let here = rates.here.as_ref().is_ok_and(|v| *v > Decimal::ZERO);
        let best = rates.better_target();
        let target = best.map(|(to, _)| (to.clone(), rates.wallet_for(to).cloned()));
        let mut reply = super::sale_price_text::render(self, &deal, qty, &rates);
        let ttl = match self.quote_ttl() {
            Ok(ttl) if started.elapsed() <= ttl => ttl,
            Ok(_) => {
                reply
                    .text
                    .push_str("\nРасчёт устарел. Запросите цену ещё раз.");
                return Ok(reply);
            }
            Err(e) => {
                reply
                    .text
                    .push_str(&format!("\nКнопки продажи недоступны: {e}"));
                return Ok(reply);
            }
        };
        let id = super::ids::uuid_v4()?;
        let action = |away| ManualAction::QuotedSale {
            record: key.into(),
            quote_id: id.clone(),
            away,
        };
        reply.here = here.then(|| action(false));
        reply.away = target.as_ref().map(|_| action(true));
        let mut tickets = lk(&self.inner.sale_quotes);
        tickets.retain(|_, t| t.started.elapsed() <= ttl);
        if tickets.get(key).is_some_and(|t| t.started > started) {
            return Err(STALE.into());
        }
        let here_wallet = rates.wallet_for(&deal.at).cloned();
        tickets.insert(
            key.into(),
            Ticket {
                id,
                started,
                deal,
                qty,
                here,
                here_wallet,
                target,
            },
        );
        Ok(reply)
    }

    pub(super) async fn execute_sale_price(
        &self,
        key: &str,
        id: &str,
        away: bool,
    ) -> Result<String, String> {
        let ttl = self.quote_ttl()?;
        let ticket = lk(&self.inner.sale_quotes)
            .get(key)
            .filter(|t| t.id == id && t.started.elapsed() <= ttl)
            .cloned()
            .ok_or(STALE)?;
        let (kind, current) = record(self, key)?;
        if !manual_price_exec::matches(&ticket.deal, &current) {
            return Err(STALE.into());
        }
        let (qty, rates) = estimate(self, &current).await?;
        if qty != ticket.qty {
            return Err(STALE.into());
        }
        let (to, wallet) = if away {
            let (to, wallet) = ticket.target.as_ref().ok_or(STALE)?;
            if rates.better_target().map(|(v, _)| v) != Some(to)
                || rates.wallet_for(to) != wallet.as_ref()
            {
                return Err(STALE.into());
            }
            (to, wallet.as_ref())
        } else {
            if !ticket.here
                || rates.wallet_for(&current.at) != ticket.here_wallet.as_ref()
                || !rates.here.as_ref().is_ok_and(|v| *v > Decimal::ZERO)
            {
                return Err(STALE.into());
            }
            (&current.at, ticket.here_wallet.as_ref())
        };
        {
            let mut tickets = lk(&self.inner.sale_quotes);
            if !tickets
                .get(key)
                .is_some_and(|t| t.id == id && t.started.elapsed() <= ttl)
            {
                return Err(STALE.into());
            }
            tickets.remove(key);
        }
        let (_, object) = key.split_once('.').ok_or(STALE)?;
        manual_price_exec::execute(self, kind, object, &current, qty, (to, wallet)).await
    }
}
