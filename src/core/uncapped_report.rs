//! Файл: core/uncapped_report.rs
//! Что делает: решение 215 — данные отчёта Excel «Упущено» (arb_missed_<дата>.xlsx) за период: те же итоги, что у экрана
//! «Упущено из-за потолка» (Core::uncapped_totals — одна выборка и один расчёт), почему расчёт не идёт, и показные имена
//! монет по адресу (решение 211) — как на экране (ui/src/lib/coinNames.svelte.js coinName: тикер монеты записи, затем
//! показной тикер справочника и тикеры площадок без повторов, через « / »).
//! Решение 245: и контекст показа окон максимального объёма (maxwin_view.rs: длительности сделок учёта, настройки).
//! Кто вызывает: report/mod.rs (missed_file).
//! От чего зависит: core/uncapped_view.rs (uncapped_totals, uncapped_off), uncapped_store.rs (ts), справочник монет ядра.
//! Настройки: uncapped.enabled, uncapped.ladder_usdt (через uncapped_totals).

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::core::types::Asset;

use super::engine::Core;
use super::uncapped_store::ts;
use super::uncapped_sum::Totals;

/// Данные отчёта «Упущено».
pub struct MissedData {
    pub totals: Totals,
    /// Почему нет таблицы потолков (uncapped.ladder_usdt не задана или неверна); None — есть.
    pub ladder_error: Option<String>,
    /// Почему расчёт без потолка не идёт (uncapped.enabled); None — идёт.
    pub off: Option<String>,
    /// Монета → показное имя «NMT / NETMIND».
    pub labels: HashMap<Asset, String>,
    /// Решение 245: показ окон максимального объёма и длительности сделок (как на экране).
    pub maxwin: super::maxwin_view::Ctx,
}

impl Core {
    /// Данные отчёта «Упущено» за период [from, to) (None — без границы). Ошибки: текст сбоя базы.
    pub async fn uncapped_report(
        &self,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Result<MissedData, String> {
        let (totals, ladder_error) = self.uncapped_totals(from.map(ts), to.map(ts)).await?;
        Ok(self.missed_data(totals, ladder_error, self.maxwin_ctx()))
    }

    /// Суточный отчёт использует отдельный читатель и сохранённые сделки всех книг этой рассылки.
    pub(crate) async fn uncapped_report_archive(
        &self,
        db: &crate::storage::db::Db,
        deals: &std::sync::Arc<Vec<super::deal::Deal>>,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Result<MissedData, String> {
        let (totals, error) = self
            .uncapped_totals_from(db, Some(deals.clone()), from.map(ts), to.map(ts))
            .await?;
        let core = self.clone();
        let deals = deals.clone();
        tokio::task::spawn_blocking(move || {
            let times = super::deal_time::DealTimes::of(deals.iter());
            core.missed_data(totals, error, core.maxwin_ctx_with_times(times))
        })
        .await
        .map_err(|e| format!("данные отчёта «Упущено»: {e}"))
    }

    /// Дополняет итоги имёнами монет и условиями расчёта на момент выгрузки.
    fn missed_data(
        &self,
        totals: Totals,
        ladder_error: Option<String>,
        maxwin: super::maxwin_view::Ctx,
    ) -> MissedData {
        let labels = self.coin_labels(totals.items.iter().map(|i| &i.b.direction.asset));
        MissedData {
            totals,
            ladder_error,
            off: self.uncapped_off(),
            labels,
            maxwin,
        }
    }

    /// Показные имена монет (см. шапку) одним чтением справочника. Принимает: монеты. Возвращает: монета → имя.
    /// Монета без адреса или вне справочника — её тикер. Ошибок нет.
    pub fn coin_labels<'a>(
        &self,
        assets: impl Iterator<Item = &'a Asset>,
    ) -> HashMap<Asset, String> {
        let coins = self.coins();
        let mut out: HashMap<Asset, String> = HashMap::new();
        for a in assets {
            if out.contains_key(a) {
                continue;
            }
            let mut names = vec![a.ticker.clone()];
            if a.contract.is_some()
                && let Some(c) = coins.all().find(|c| c.asset == *a)
            {
                let mut venues: Vec<_> = c.presence.iter().collect();
                venues.sort_by(|x, y| x.venue.0.cmp(&y.venue.0));
                for t in std::iter::once(&c.asset.ticker).chain(venues.iter().map(|p| &p.ticker)) {
                    if !names.contains(t) {
                        names.push(t.clone());
                    }
                }
            }
            out.insert(a.clone(), names.join(" / "));
        }
        out
    }
}
