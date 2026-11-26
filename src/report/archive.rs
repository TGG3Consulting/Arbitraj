//! История для суточной рассылки. Все книги серии используют одну копию сохранённых сделок.
//! Свои соединения чтения основной базы и учёта позволяют SQLite читать WAL параллельно с писателями.

use std::cmp::Reverse;
use std::sync::Arc;

use crate::core::{Core, deal::Deal, deal_return_cost, ledger, uncapped_report::MissedData};
use crate::storage::db::Db;
use chrono::{DateTime, Utc};
use tokio::sync::OnceCell;

pub(crate) struct Archive {
    pub db: Db,
    pub deals: Arc<Vec<Deal>>,
    missed: OnceCell<PeriodMissed>,
}

type PeriodMissed = (
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Arc<MissedData>,
);

impl Archive {
    /// Открывает читатели после захвата задания рассылки. Ошибки файла и повреждённые сделки видны отправителю.
    pub async fn open(core: &Core) -> Result<Self, String> {
        let db = core
            .inner
            .deps
            .db
            .report_reader()
            .await
            .map_err(|e| e.to_string())?;
        let deals = db
            .ledger()
            .read(|c| {
                let mut stmt = c.prepare("SELECT id, body FROM core_state WHERE kind = ?1")?;
                let mut rows = stmt.query([ledger::KIND])?;
                let mut deals = Vec::new();
                while let Some(row) = rows.next()? {
                    let id: String = row.get(0)?;
                    let body: String = row.get(1)?;
                    let mut deal: Deal = match serde_json::from_str(&body) {
                        Ok(deal) => deal,
                        Err(e) => return Ok(Err(format!("сделка {id}: {e}"))),
                    };
                    // Та же поправка старых записей на стоимость возврата, что при загрузке ядра.
                    deal_return_cost::apply(&mut deal);
                    deals.push(deal);
                }
                deals.sort_by_key(|d| Reverse(d.started_at));
                Ok(Ok(deals))
            })
            .await
            .map_err(|e| e.to_string())??;
        Ok(Self {
            db,
            deals: Arc::new(deals),
            missed: OnceCell::new(),
        })
    }

    /// Одни и те же итоги периода для книг «Упущено» и «Капитал». Ошибка чтения не сохраняется.
    pub async fn missed(
        &self,
        core: &Core,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Result<Arc<MissedData>, String> {
        let (a, b, data) = self
            .missed
            .get_or_try_init(|| async {
                let data = core
                    .uncapped_report_archive(&self.db, &self.deals, from, to)
                    .await?;
                Ok::<_, String>((from, to, Arc::new(data)))
            })
            .await?;
        if *a == from && *b == to {
            return Ok(data.clone());
        }
        core.uncapped_report_archive(&self.db, &self.deals, from, to)
            .await
            .map(Arc::new)
    }
}
